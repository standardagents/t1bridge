//! Single-threaded privileged Touch Bar hardware service.

use std::env;
use std::fmt;
use std::os::fd::{AsFd, OwnedFd};
use std::time::{Duration, Instant};

use t1_platform::diagnostics::{Component, Stage, observe};
use t1_platform::frame_memfd::ReadOnlyFrame;
use t1_platform::seqpacket::{
    SeqPacketClient, SeqPacketError, SystemdActivation, SystemdSeqPacketListener,
};
use t1_platform::system_brightness::{BrightnessKind, BrightnessStep, SystemBrightnessControls};
use t1_platform::touchbar_drm::{TouchBarDamageRectangle, TouchBarDisplay, TouchBarDrmError};
use t1_platform::touchbar_io::{
    ClientInterest, DigitizerRead, EventWait, FnRead, ReadyInputs, TouchBarDigitizer,
    TouchBarFnInput, TouchBarKey, TouchBarKeyboard, wait_events,
};
use t1_platform::touchbar_session::{
    SessionBrightnessKind, SessionRevalidation, TouchBarSessionWatch,
};

use crate::digitizer::{DisplayDimensions, parse_digitizer_payload};
use crate::frame_state::{FrameLayout, FrameState, FrameStateError, VerifiedBufferMetadata};
use crate::input_state::{InputFrame, InputState};
use crate::packet::MAX_PACKET_LENGTH;
use crate::touchid_cancel::{CancelTouchIdOutcome, cancel_touch_id};
use crate::wire::{
    AncillaryMetadata, ClientMessage, Envelope, ErrorCode, FeatureBits, FrameReleased, HelloAck,
    InputContact, KeyCode, PixelFormat, ServiceMessage, SubmitFrame, WireInputFrame, decode_client,
    encode_service,
};

const SOCKET_PATH: &std::ffi::CStr = c"/run/t1bridge/touchbar.sock";
const INPUT_WAIT_MS: u32 = 10;
const SESSION_REVALIDATE_INTERVAL: Duration = Duration::from_millis(50);
const BRIGHTNESS_ACTION_INTERVAL: Duration = Duration::from_millis(20);
const MAX_INPUT_DRAIN: usize = 32;
const BASE_SERVICE_FEATURES: FeatureBits = FeatureBits::from_bits(
    FeatureBits::INITIAL_SERVICE.bits() | FeatureBits::TOUCH_ID_CANCELLATION.bits(),
);

struct HardwareActions {
    brightness: SystemBrightnessControls,
    display_last: Option<Instant>,
    keyboard_last: Option<Instant>,
}

impl HardwareActions {
    fn discover() -> Self {
        Self {
            brightness: SystemBrightnessControls::discover(),
            display_last: None,
            keyboard_last: None,
        }
    }

    fn features(&self) -> FeatureBits {
        let mut bits = BASE_SERVICE_FEATURES.bits();
        if self.brightness.available(BrightnessKind::Display) {
            bits |= FeatureBits::DISPLAY_BRIGHTNESS.bits();
        }
        if self.brightness.available(BrightnessKind::Keyboard) {
            bits |= FeatureBits::KEYBOARD_BACKLIGHT.bits();
        }
        FeatureBits::from_bits(bits)
    }

    fn set_brightness(
        &mut self,
        kind: BrightnessKind,
        percentage: u8,
        session: &mut TouchBarSessionWatch,
    ) -> ServiceMessage {
        if !self.brightness.available(kind) {
            return ServiceMessage::Error(ErrorCode::UnsupportedFeature);
        }
        let last = match kind {
            BrightnessKind::Display => &mut self.display_last,
            BrightnessKind::Keyboard => &mut self.keyboard_last,
        };
        let now = Instant::now();
        if last.is_some_and(|previous| now.duration_since(previous) < BRIGHTNESS_ACTION_INTERVAL) {
            return ServiceMessage::Error(ErrorCode::ResourceLimit);
        }
        *last = Some(now);
        let Ok(setting) = self.brightness.setting(kind, percentage) else {
            return ServiceMessage::Error(ErrorCode::IoFailure);
        };
        let session_kind = match setting.kind {
            BrightnessKind::Display => SessionBrightnessKind::Display,
            BrightnessKind::Keyboard => SessionBrightnessKind::Keyboard,
        };
        if session
            .set_brightness(session_kind, setting.device_name, setting.value)
            .is_ok()
        {
            ServiceMessage::Ack
        } else {
            self.brightness.disable(kind);
            ServiceMessage::Error(ErrorCode::IoFailure)
        }
    }

    fn step_brightness(
        &mut self,
        kind: BrightnessKind,
        direction: crate::wire::StepDirection,
        session: &mut TouchBarSessionWatch,
    ) -> ServiceMessage {
        if !self.brightness.available(kind) {
            return ServiceMessage::Error(ErrorCode::UnsupportedFeature);
        }
        let last = match kind {
            BrightnessKind::Display => &mut self.display_last,
            BrightnessKind::Keyboard => &mut self.keyboard_last,
        };
        let now = Instant::now();
        if last.is_some_and(|previous| now.duration_since(previous) < BRIGHTNESS_ACTION_INTERVAL) {
            return ServiceMessage::Error(ErrorCode::ResourceLimit);
        }
        *last = Some(now);
        let platform_direction = match direction {
            crate::wire::StepDirection::Down => BrightnessStep::Down,
            crate::wire::StepDirection::Up => BrightnessStep::Up,
        };
        let Ok(setting) = self.brightness.step_setting(kind, platform_direction) else {
            return ServiceMessage::Error(ErrorCode::IoFailure);
        };
        let session_kind = match setting.kind {
            BrightnessKind::Display => SessionBrightnessKind::Display,
            BrightnessKind::Keyboard => SessionBrightnessKind::Keyboard,
        };
        if session
            .set_brightness(session_kind, setting.device_name, setting.value)
            .is_ok()
        {
            ServiceMessage::Ack
        } else {
            self.brightness.disable(kind);
            ServiceMessage::Error(ErrorCode::IoFailure)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceError {
    Activation,
    Listener,
    Display(TouchBarDrmError),
    Input,
    Keyboard,
    Session,
    Geometry,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Activation => "socket activation failed",
            Self::Listener => "local listener failed",
            Self::Display(error) => return write!(formatter, "Touch Bar display failed: {error}"),
            Self::Input => "Touch Bar input failed",
            Self::Keyboard => "Touch Bar keyboard failed",
            Self::Session => "session monitor failed",
            Self::Geometry => "Touch Bar geometry is invalid",
        })
    }
}

impl std::error::Error for ServiceError {}

struct RegisteredBuffer {
    id: u32,
    frame: ReadOnlyFrame,
}

struct PendingPacket {
    bytes: Vec<u8>,
    after_send: AfterSend,
}

enum AfterSend {
    None,
    Establish(u16),
    Present {
        buffer_id: u32,
        frame_id: u64,
        damage: Vec<crate::frame_state::DamageRectangle>,
    },
    Release {
        buffer_id: u32,
        frame_id: u64,
        close_after: bool,
    },
    Close,
}

struct Connection {
    descriptor: OwnedFd,
    established: bool,
    negotiated_minor: Option<u16>,
    frames: FrameState<u32, u64>,
    buffers: Vec<RegisteredBuffer>,
    control: Option<PendingPacket>,
    input: Option<Vec<u8>>,
    needs_full_present: bool,
}

struct RequestServices<'a> {
    dimensions: DisplayDimensions,
    layout: FrameLayout,
    keyboard: &'a mut TouchBarKeyboard,
    actions: &'a mut HardwareActions,
    session: &'a mut TouchBarSessionWatch,
}

impl Connection {
    fn new(descriptor: OwnedFd, layout: FrameLayout) -> Self {
        Self {
            descriptor,
            established: false,
            negotiated_minor: None,
            frames: FrameState::new(layout),
            buffers: Vec::new(),
            control: None,
            input: None,
            needs_full_present: true,
        }
    }

    fn client(&self) -> SeqPacketClient<'_> {
        SeqPacketClient::new(self.descriptor.as_fd())
    }

    fn phase_dimensions(&self, dimensions: DisplayDimensions) -> Option<DisplayDimensions> {
        self.established.then_some(dimensions)
    }

    fn queue_control(
        &mut self,
        request_id: u32,
        message: ServiceMessage,
        dimensions: Option<DisplayDimensions>,
        after_send: AfterSend,
    ) -> Result<(), ()> {
        let bytes = encode_service(
            &Envelope {
                request_id,
                message,
            },
            dimensions,
        )
        .map_err(|_| ())?;
        self.control = Some(PendingPacket { bytes, after_send });
        Ok(())
    }

    fn queue_error(
        &mut self,
        request_id: u32,
        code: ErrorCode,
        dimensions: DisplayDimensions,
    ) -> Result<(), ()> {
        self.queue_control(
            request_id,
            ServiceMessage::Error(code),
            self.phase_dimensions(dimensions),
            AfterSend::None,
        )
    }

    fn queue_input(&mut self, frame: &InputFrame, dimensions: DisplayDimensions) {
        if !self.established {
            return;
        }
        let message = ServiceMessage::InputFrame(normalize_input(frame, dimensions));
        if let Ok(bytes) = encode_service(
            &Envelope {
                request_id: 0,
                message,
            },
            Some(dimensions),
        ) {
            self.input = Some(bytes);
        }
    }

    fn disconnect(&mut self) {
        self.frames.disconnect();
        self.buffers.clear();
        self.control = None;
        self.input = None;
    }
}

/// Runs the production root hardware service until a fatal hardware or
/// activation error occurs.
///
/// # Errors
///
/// Returns only redaction-safe, static failure categories.
pub fn run() -> Result<(), ServiceError> {
    let listen_pid = env::var("LISTEN_PID").map_err(|_| ServiceError::Activation)?;
    let listen_fds = env::var("LISTEN_FDS").map_err(|_| ServiceError::Activation)?;
    let activation =
        SystemdActivation::parse(&listen_pid, &listen_fds).map_err(|_| ServiceError::Activation)?;
    let listener = SystemdSeqPacketListener::adopt(activation, SOCKET_PATH)
        .map_err(|_| ServiceError::Listener)?;

    let mut display = observe_hardware(Stage::DisplayOpen, TouchBarDisplay::open)
        .map_err(ServiceError::Display)?;
    let (dimensions, layout, mut snapshot) = frame_setup(&display)?;

    let mut digitizer = observe_hardware(Stage::DigitizerOpen, TouchBarDigitizer::open)
        .map_err(|_| ServiceError::Input)?;
    let mut function =
        observe_hardware(Stage::FnOpen, TouchBarFnInput::open).map_err(|_| ServiceError::Input)?;
    let initial_fn = function.pressed().map_err(|_| ServiceError::Input)?;
    let mut keyboard = observe_hardware(Stage::KeyboardCreate, TouchBarKeyboard::create)
        .map_err(|_| ServiceError::Keyboard)?;
    let mut actions = HardwareActions::discover();
    let mut session = observe_hardware(Stage::SessionWatch, TouchBarSessionWatch::new)
        .map_err(|_| ServiceError::Session)?;
    let origin = Instant::now();
    let mut last_session_check = origin;
    let mut input_state = InputState::new(dimensions, initial_fn);
    let mut connection: Option<Connection> = None;
    let mut receive_buffer = vec![0_u8; MAX_PACKET_LENGTH];

    loop {
        if connection.is_some() && last_session_check.elapsed() >= SESSION_REVALIDATE_INTERVAL {
            last_session_check = Instant::now();
            if !matches!(session.refresh(), Ok(SessionRevalidation::Retained)) {
                revoke(
                    &mut connection,
                    &mut session,
                    &mut keyboard,
                    &mut input_state,
                    origin,
                )?;
            }
        }

        if let Some(active) = connection.as_mut()
            && flush_connection(active, &mut display, &mut snapshot, dimensions).is_err()
        {
            t1_platform::diagnostics::emit(t1_platform::diagnostics::Record::new(
                Component::TouchbarHardware,
                Stage::Frame,
                t1_platform::diagnostics::Outcome::Error,
                None,
            ));
            revoke(
                &mut connection,
                &mut session,
                &mut keyboard,
                &mut input_state,
                origin,
            )?;
        }

        if let Some(active) = connection.as_mut()
            && active.control.is_none()
            && receive_request(
                active,
                &mut receive_buffer,
                RequestServices {
                    dimensions,
                    layout,
                    keyboard: &mut keyboard,
                    actions: &mut actions,
                    session: &mut session,
                },
            )
            .is_err()
        {
            revoke(
                &mut connection,
                &mut session,
                &mut keyboard,
                &mut input_state,
                origin,
            )?;
        }

        if connection.is_none()
            && listener.is_ready().map_err(|_| ServiceError::Listener)?
            && let Ok(descriptor) = listener.listener().accept()
            && observe(Component::TouchbarHardware, Stage::SessionAdmission, || {
                admit(&descriptor, &mut session)
            })
            .is_ok()
        {
            connection = Some(Connection::new(descriptor, layout));
            last_session_check = Instant::now();
        }

        let ready = wait_for_service_events(&digitizer, &function, connection.as_ref())?;
        process_input(
            ready,
            &mut digitizer,
            &mut function,
            &mut input_state,
            &mut connection,
            dimensions,
            origin,
        )?;
    }
}

fn wait_for_service_events(
    digitizer: &TouchBarDigitizer,
    function: &TouchBarFnInput,
    connection: Option<&Connection>,
) -> Result<ReadyInputs, ServiceError> {
    let client = connection.map(|active| {
        (
            active.descriptor.as_fd(),
            client_interest(active.control.is_some(), active.input.is_some()),
        )
    });
    match wait_events(digitizer, function, client, INPUT_WAIT_MS)
        .map_err(|_| ServiceError::Input)?
    {
        EventWait::Ready(events) => Ok(events.inputs),
        EventWait::Idle => Ok(ReadyInputs::default()),
    }
}

fn frame_setup(
    display: &TouchBarDisplay,
) -> Result<(DisplayDimensions, FrameLayout, Vec<u8>), ServiceError> {
    let geometry = display.geometry();
    let layout = FrameLayout::new(geometry.width(), geometry.height())
        .map_err(|_| ServiceError::Geometry)?;
    if geometry.stride() != layout.stride() || geometry.byte_length() != layout.byte_length() {
        return Err(ServiceError::Geometry);
    }
    let dimensions = DisplayDimensions::new(layout.width(), layout.height())
        .map_err(|_| ServiceError::Geometry)?;
    let snapshot_length =
        usize::try_from(layout.byte_length()).map_err(|_| ServiceError::Geometry)?;
    Ok((dimensions, layout, vec![0_u8; snapshot_length]))
}

#[allow(clippy::too_many_arguments)]
fn process_input(
    ready: ReadyInputs,
    digitizer: &mut TouchBarDigitizer,
    function: &mut TouchBarFnInput,
    input_state: &mut InputState,
    connection: &mut Option<Connection>,
    dimensions: DisplayDimensions,
    origin: Instant,
) -> Result<(), ServiceError> {
    if ready.digitizer {
        for _ in 0..MAX_INPUT_DRAIN {
            match digitizer.read(0).map_err(|_| ServiceError::Input)? {
                DigitizerRead::Report(bytes) => {
                    let report =
                        parse_digitizer_payload(&bytes).map_err(|_| ServiceError::Input)?;
                    let now = Instant::now();
                    let frame = input_state.ingest_touch(now.duration_since(origin), &report);
                    if let Some(active) = connection.as_mut() {
                        active.queue_input(&frame, dimensions);
                    }
                }
                DigitizerRead::Idle => break,
            }
        }
    }
    if ready.function {
        match function.read().map_err(|_| ServiceError::Input)? {
            FnRead::Edges(edges) => {
                for edge in edges {
                    if let Some(frame) = input_state.set_fn_pressed(origin.elapsed(), edge.pressed)
                        && let Some(active) = connection.as_mut()
                    {
                        active.queue_input(&frame, dimensions);
                    }
                }
            }
            FnRead::Resync => {
                let pressed = function.pressed().map_err(|_| ServiceError::Input)?;
                if let Some(frame) = input_state.set_fn_pressed(origin.elapsed(), pressed)
                    && let Some(active) = connection.as_mut()
                {
                    active.queue_input(&frame, dimensions);
                }
            }
            FnRead::Idle => {}
        }
    } else {
        // A key remapper such as keyd holds the keyboard with EVIOCGRAB, so no
        // Fn events reach this descriptor, but the kernel's key state still
        // tracks Fn. set_fn_pressed ignores an unchanged state. This only runs
        // because wait_events returns at least every INPUT_WAIT_MS: if the loop
        // ever sleeps until an event arrives, Fn stops working under a grab.
        let pressed = function.pressed().map_err(|_| ServiceError::Input)?;
        if let Some(frame) = input_state.set_fn_pressed(origin.elapsed(), pressed)
            && let Some(active) = connection.as_mut()
        {
            active.queue_input(&frame, dimensions);
        }
    }
    Ok(())
}

const fn client_interest(has_control: bool, has_input: bool) -> ClientInterest {
    ClientInterest {
        readable: !has_control,
        writable: has_control || has_input,
    }
}

fn observe_hardware<T, E>(stage: Stage, operation: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    observe(Component::TouchbarHardware, stage, operation)
}

fn admit(descriptor: &OwnedFd, session: &mut TouchBarSessionWatch) -> Result<(), ()> {
    let client = SeqPacketClient::new(descriptor.as_fd());
    let credentials = client.peer_credentials().map_err(|_| ())?;
    if credentials.user_id == 0 {
        return Err(());
    }
    client.require_peer_in_effective_group().map_err(|_| ())?;
    session.admit(credentials.user_id).map_err(|_| ())
}

fn receive_request(
    connection: &mut Connection,
    receive_buffer: &mut [u8],
    mut services: RequestServices<'_>,
) -> Result<(), ()> {
    let (received, descriptor) = match connection.client().receive_at_most_one_fd(receive_buffer) {
        Ok(packet) => packet,
        Err(SeqPacketError::WouldBlock | SeqPacketError::Interrupted) => return Ok(()),
        Err(_) => return Err(()),
    };
    let ancillary = AncillaryMetadata {
        rights_descriptor_count: usize::from(descriptor.is_some()),
        control_truncated: false,
        has_other_control: false,
    };
    let envelope = decode_client(
        &receive_buffer[..received],
        ancillary,
        connection.phase_dimensions(services.dimensions),
    )
    .map_err(|_| ())?;
    handle_request(connection, envelope, descriptor, &mut services)
}

fn handle_request(
    connection: &mut Connection,
    envelope: Envelope<ClientMessage>,
    descriptor: Option<OwnedFd>,
    services: &mut RequestServices<'_>,
) -> Result<(), ()> {
    let request_id = envelope.request_id;
    let dimensions = services.dimensions;
    match envelope.message {
        ClientMessage::Hello(hello) => queue_hello(
            connection,
            request_id,
            hello.required_features,
            hello.minor,
            dimensions,
            services.actions,
        ),
        ClientMessage::RegisterBuffer(buffer) => {
            let descriptor = descriptor.ok_or(())?;
            queue_buffer_registration(
                connection,
                request_id,
                dimensions,
                services.layout,
                buffer,
                &descriptor,
            )
        }
        ClientMessage::SubmitFrame(frame) => {
            queue_frame_submission(connection, request_id, dimensions, &frame)
        }
        ClientMessage::TapKeys(keys) => {
            for key in keys.keys {
                if services.keyboard.tap(platform_key(key)).is_err() {
                    if services.keyboard.release_all().is_err() {
                        return Err(());
                    }
                    return connection.queue_error(request_id, ErrorCode::IoFailure, dimensions);
                }
            }
            connection.queue_control(
                request_id,
                ServiceMessage::Ack,
                Some(dimensions),
                AfterSend::None,
            )
        }
        ClientMessage::CancelTouchId => queue_cancellation(connection, request_id, dimensions),
        ClientMessage::SetDisplayBrightness(percentage) => queue_brightness(
            connection,
            request_id,
            dimensions,
            services.actions,
            services.session,
            BrightnessKind::Display,
            percentage.value(),
        ),
        ClientMessage::SetKeyboardBacklight(percentage) => queue_brightness(
            connection,
            request_id,
            dimensions,
            services.actions,
            services.session,
            BrightnessKind::Keyboard,
            percentage.value(),
        ),
        ClientMessage::StepDisplayBrightness(direction) => queue_brightness_step(
            connection,
            request_id,
            dimensions,
            services.actions,
            services.session,
            BrightnessKind::Display,
            direction,
        ),
        ClientMessage::StepKeyboardBacklight(direction) => queue_brightness_step(
            connection,
            request_id,
            dimensions,
            services.actions,
            services.session,
            BrightnessKind::Keyboard,
            direction,
        ),
        ClientMessage::Unknown(_) => {
            connection.queue_error(request_id, ErrorCode::Unsupported, dimensions)
        }
    }
}

fn queue_buffer_registration(
    connection: &mut Connection,
    request_id: u32,
    dimensions: DisplayDimensions,
    layout: FrameLayout,
    buffer: crate::wire::RegisterBuffer,
    descriptor: &OwnedFd,
) -> Result<(), ()> {
    let Ok(frame) = ReadOnlyFrame::accept(descriptor.as_fd(), layout.byte_length()) else {
        return connection.queue_error(request_id, ErrorCode::InvalidBuffer, dimensions);
    };
    let metadata = VerifiedBufferMetadata {
        stride: buffer.stride,
        byte_length: buffer.byte_length,
    };
    match connection
        .frames
        .register_buffer(buffer.buffer_id, metadata)
    {
        Ok(()) => {
            connection.buffers.push(RegisteredBuffer {
                id: buffer.buffer_id,
                frame,
            });
            connection.queue_control(
                request_id,
                ServiceMessage::Ack,
                Some(dimensions),
                AfterSend::None,
            )
        }
        Err(error) => connection.queue_error(request_id, frame_error_code(error), dimensions),
    }
}

fn queue_frame_submission(
    connection: &mut Connection,
    request_id: u32,
    dimensions: DisplayDimensions,
    frame: &SubmitFrame,
) -> Result<(), ()> {
    match connection
        .frames
        .submit_frame(&frame.buffer_id, frame.frame_id, &frame.damage)
    {
        Ok(submission) => connection.queue_control(
            request_id,
            ServiceMessage::Ack,
            Some(dimensions),
            AfterSend::Present {
                buffer_id: submission.buffer_id,
                frame_id: submission.frame_id,
                damage: submission.damage,
            },
        ),
        Err(error) => connection.queue_error(request_id, frame_error_code(error), dimensions),
    }
}

fn queue_hello(
    connection: &mut Connection,
    request_id: u32,
    required: FeatureBits,
    client_minor: u16,
    dimensions: DisplayDimensions,
    actions: &HardwareActions,
) -> Result<(), ()> {
    if !actions.features().contains(required) {
        return connection.queue_control(
            request_id,
            ServiceMessage::Error(ErrorCode::UnsupportedFeature),
            None,
            AfterSend::Close,
        );
    }
    let negotiated_minor = client_minor.min(crate::wire::PROTOCOL_MINOR);
    connection.queue_control(
        request_id,
        ServiceMessage::HelloAck(HelloAck {
            minor: negotiated_minor,
            width: dimensions.width(),
            height: dimensions.height(),
            pixel_format: PixelFormat::Xrgb8888,
            max_buffers: 3,
        }),
        None,
        AfterSend::Establish(negotiated_minor),
    )
}

fn queue_brightness_step(
    connection: &mut Connection,
    request_id: u32,
    dimensions: DisplayDimensions,
    actions: &mut HardwareActions,
    session: &mut TouchBarSessionWatch,
    kind: BrightnessKind,
    direction: crate::wire::StepDirection,
) -> Result<(), ()> {
    if connection.negotiated_minor.unwrap_or(0) < 1 {
        return connection.queue_error(request_id, ErrorCode::Unsupported, dimensions);
    }
    connection.queue_control(
        request_id,
        actions.step_brightness(kind, direction, session),
        Some(dimensions),
        AfterSend::None,
    )
}

fn queue_brightness(
    connection: &mut Connection,
    request_id: u32,
    dimensions: DisplayDimensions,
    actions: &mut HardwareActions,
    session: &mut TouchBarSessionWatch,
    kind: BrightnessKind,
    percentage: u8,
) -> Result<(), ()> {
    connection.queue_control(
        request_id,
        actions.set_brightness(kind, percentage, session),
        Some(dimensions),
        AfterSend::None,
    )
}

fn queue_cancellation(
    connection: &mut Connection,
    request_id: u32,
    dimensions: DisplayDimensions,
) -> Result<(), ()> {
    connection.queue_control(
        request_id,
        cancellation_response(cancel_touch_id()),
        Some(dimensions),
        AfterSend::None,
    )
}

fn cancellation_response(
    result: Result<CancelTouchIdOutcome, crate::touchid_cancel::CancelTouchIdError>,
) -> ServiceMessage {
    match result {
        Ok(CancelTouchIdOutcome::Delivered) => ServiceMessage::Ack,
        Ok(CancelTouchIdOutcome::Denied) => ServiceMessage::Error(ErrorCode::ActionDenied),
        Err(_) => ServiceMessage::Error(ErrorCode::IoFailure),
    }
}

fn flush_connection(
    connection: &mut Connection,
    display: &mut TouchBarDisplay,
    snapshot: &mut [u8],
    dimensions: DisplayDimensions,
) -> Result<(), ()> {
    if let Some(pending) = connection.control.take() {
        match connection.client().send(&pending.bytes) {
            Ok(()) => apply_after_send(
                connection,
                pending.after_send,
                display,
                snapshot,
                dimensions,
            )?,
            Err(SeqPacketError::WouldBlock | SeqPacketError::Interrupted) => {
                connection.control = Some(pending);
                return Ok(());
            }
            Err(_) => return Err(()),
        }
    }
    if connection.control.is_none()
        && let Some(input) = connection.input.take()
    {
        match connection.client().send(&input) {
            Ok(()) => {}
            Err(SeqPacketError::WouldBlock | SeqPacketError::Interrupted) => {
                connection.input = Some(input);
            }
            Err(_) => return Err(()),
        }
    }
    Ok(())
}

fn apply_after_send(
    connection: &mut Connection,
    action: AfterSend,
    display: &mut TouchBarDisplay,
    snapshot: &mut [u8],
    dimensions: DisplayDimensions,
) -> Result<(), ()> {
    match action {
        AfterSend::None => Ok(()),
        AfterSend::Establish(minor) => {
            connection.established = true;
            connection.negotiated_minor = Some(minor);
            Ok(())
        }
        AfterSend::Close => Err(()),
        AfterSend::Present {
            buffer_id,
            frame_id,
            mut damage,
        } => {
            expand_first_present_damage(
                &mut connection.needs_full_present,
                &mut damage,
                dimensions,
            );
            let mut native_damage = [TouchBarDamageRectangle {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }; crate::wire::MAX_DAMAGE_RECTANGLES];
            for (native, rectangle) in native_damage.iter_mut().zip(&damage) {
                *native = TouchBarDamageRectangle {
                    x: rectangle.x,
                    y: rectangle.y,
                    width: rectangle.width,
                    height: rectangle.height,
                };
            }
            let presented = connection
                .buffers
                .iter()
                .find(|buffer| buffer.id == buffer_id)
                .ok_or(())
                .and_then(|buffer| buffer.frame.copy_into(snapshot).map_err(|_| ()))
                .and_then(|()| {
                    display
                        .present_rectangles(snapshot, &native_damage[..damage.len()])
                        .map_err(|_| ())
                })
                .is_ok();
            connection.queue_control(
                0,
                ServiceMessage::FrameReleased(FrameReleased {
                    buffer_id,
                    frame_id,
                }),
                Some(dimensions),
                AfterSend::Release {
                    buffer_id,
                    frame_id,
                    close_after: !presented,
                },
            )
        }
        AfterSend::Release {
            buffer_id,
            frame_id,
            close_after,
        } => {
            connection
                .frames
                .release_frame(&buffer_id, &frame_id)
                .map_err(|_| ())?;
            if close_after { Err(()) } else { Ok(()) }
        }
    }
}

fn expand_first_present_damage(
    needs_full_present: &mut bool,
    damage: &mut Vec<crate::frame_state::DamageRectangle>,
    dimensions: DisplayDimensions,
) {
    if std::mem::replace(needs_full_present, false) {
        damage.clear();
        damage.push(crate::frame_state::DamageRectangle {
            x: 0,
            y: 0,
            width: dimensions.width(),
            height: dimensions.height(),
        });
    }
}

fn revoke(
    connection: &mut Option<Connection>,
    session: &mut TouchBarSessionWatch,
    keyboard: &mut TouchBarKeyboard,
    input_state: &mut InputState,
    origin: Instant,
) -> Result<(), ServiceError> {
    t1_platform::diagnostics::emit(t1_platform::diagnostics::Record::new(
        Component::TouchbarHardware,
        Stage::SessionRevocation,
        t1_platform::diagnostics::Outcome::Begin,
        None,
    ));
    if let Some(mut active) = connection.take() {
        active.disconnect();
    }
    let released = keyboard.release_all().is_ok();
    session.release();
    let _ = input_state.touch_idle(origin.elapsed());
    if !released {
        return Err(ServiceError::Keyboard);
    }
    Ok(())
}

fn normalize_input(frame: &InputFrame, dimensions: DisplayDimensions) -> WireInputFrame {
    let max_x = dimensions.width() - 1;
    let max_y = dimensions.height() - 1;
    WireInputFrame {
        monotonic_ns: u64::try_from(frame.timestamp.as_nanos()).unwrap_or(u64::MAX),
        fn_pressed: frame.fn_pressed,
        contacts: frame
            .contacts
            .iter()
            .map(|contact| InputContact {
                id: contact.id,
                tip: contact.tip,
                in_range: contact.in_range,
                x: rounded_coordinate(contact.x, max_x),
                y: rounded_coordinate(contact.y, max_y),
            })
            .collect(),
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rounded_coordinate(value: f64, maximum: u32) -> u32 {
    value.round().clamp(0.0, f64::from(maximum)) as u32
}

const fn platform_key(key: KeyCode) -> TouchBarKey {
    match key {
        KeyCode::Escape => TouchBarKey::Escape,
        KeyCode::F1 => TouchBarKey::F1,
        KeyCode::F2 => TouchBarKey::F2,
        KeyCode::F3 => TouchBarKey::F3,
        KeyCode::F4 => TouchBarKey::F4,
        KeyCode::F5 => TouchBarKey::F5,
        KeyCode::F6 => TouchBarKey::F6,
        KeyCode::F7 => TouchBarKey::F7,
        KeyCode::F8 => TouchBarKey::F8,
        KeyCode::F9 => TouchBarKey::F9,
        KeyCode::F10 => TouchBarKey::F10,
        KeyCode::F11 => TouchBarKey::F11,
        KeyCode::F12 => TouchBarKey::F12,
    }
}

const fn frame_error_code(error: FrameStateError) -> ErrorCode {
    match error {
        FrameStateError::TooManyBuffers => ErrorCode::ResourceLimit,
        FrameStateError::UnknownBuffer => ErrorCode::UnknownBuffer,
        FrameStateError::BufferInFlight => ErrorCode::BufferBusy,
        FrameStateError::InvalidLayout
        | FrameStateError::InvalidBufferStride
        | FrameStateError::InvalidBufferLength
        | FrameStateError::DuplicateBuffer
        | FrameStateError::TooManyDamageRectangles
        | FrameStateError::DamageOutOfBounds
        | FrameStateError::BufferNotInFlight
        | FrameStateError::FrameMismatch => ErrorCode::InvalidBuffer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_only_the_wire_key_allowlist() {
        assert_eq!(platform_key(KeyCode::Escape), TouchBarKey::Escape);
        assert_eq!(platform_key(KeyCode::F1), TouchBarKey::F1);
        assert_eq!(platform_key(KeyCode::F12), TouchBarKey::F12);
    }

    #[test]
    fn frame_failures_have_stable_public_categories() {
        assert_eq!(
            frame_error_code(FrameStateError::TooManyBuffers),
            ErrorCode::ResourceLimit
        );
        assert_eq!(
            frame_error_code(FrameStateError::UnknownBuffer),
            ErrorCode::UnknownBuffer
        );
        assert_eq!(
            frame_error_code(FrameStateError::BufferInFlight),
            ErrorCode::BufferBusy
        );
        assert_eq!(
            frame_error_code(FrameStateError::DuplicateBuffer),
            ErrorCode::InvalidBuffer
        );
    }

    #[test]
    fn base_features_and_cancellation_results_are_fixed() {
        assert!(BASE_SERVICE_FEATURES.contains(FeatureBits::INITIAL_SERVICE));
        assert!(BASE_SERVICE_FEATURES.contains(FeatureBits::TOUCH_ID_CANCELLATION));
        assert!(!BASE_SERVICE_FEATURES.contains(FeatureBits::DISPLAY_BRIGHTNESS));
        assert!(!BASE_SERVICE_FEATURES.contains(FeatureBits::KEYBOARD_BACKLIGHT));

        assert_eq!(
            cancellation_response(Ok(CancelTouchIdOutcome::Delivered)),
            ServiceMessage::Ack
        );
        assert_eq!(
            cancellation_response(Ok(CancelTouchIdOutcome::Denied)),
            ServiceMessage::Error(ErrorCode::ActionDenied)
        );
        assert_eq!(
            cancellation_response(Err(crate::touchid_cancel::CancelTouchIdError)),
            ServiceMessage::Error(ErrorCode::IoFailure)
        );
    }

    #[test]
    fn client_wait_interest_tracks_service_queues() {
        assert_eq!(
            client_interest(false, false),
            ClientInterest {
                readable: true,
                writable: false,
            }
        );
        assert_eq!(
            client_interest(true, false),
            ClientInterest {
                readable: false,
                writable: true,
            }
        );
        assert_eq!(
            client_interest(false, true),
            ClientInterest {
                readable: true,
                writable: true,
            }
        );
    }

    #[test]
    fn first_present_per_connection_expands_to_the_full_frame() {
        let dimensions = DisplayDimensions::new(2170, 60).expect("valid dimensions");
        let partial = vec![crate::frame_state::DamageRectangle {
            x: 100,
            y: 10,
            width: 40,
            height: 20,
        }];
        let mut needs_full_present = true;

        let mut first = partial.clone();
        expand_first_present_damage(&mut needs_full_present, &mut first, dimensions);
        assert_eq!(
            first,
            vec![crate::frame_state::DamageRectangle {
                x: 0,
                y: 0,
                width: 2170,
                height: 60,
            }]
        );
        let mut second = partial.clone();
        expand_first_present_damage(&mut needs_full_present, &mut second, dimensions);
        assert_eq!(second, partial);

        let mut next_connection_needs_full_present = true;
        let mut next = vec![crate::frame_state::DamageRectangle {
            x: 2169,
            y: 59,
            width: 1,
            height: 1,
        }];
        expand_first_present_damage(
            &mut next_connection_needs_full_present,
            &mut next,
            dimensions,
        );
        assert_eq!(
            next,
            vec![crate::frame_state::DamageRectangle {
                x: 0,
                y: 0,
                width: 2170,
                height: 60,
            }]
        );
    }
}
