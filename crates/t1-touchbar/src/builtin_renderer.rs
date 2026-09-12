//! Production built-in renderer for the first private Touch Bar cutover.

use std::collections::BTreeMap;
use std::fmt;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use t1_platform::diagnostics::{Component, Outcome, Record, Stage, emit};
use t1_platform::frame_memfd::RendererFrame;
use t1_platform::seqpacket::{self, SeqPacketClient, SeqPacketError};
use t1_touchbar_hw::digitizer::DisplayDimensions;
use t1_touchbar_hw::packet::MAX_PACKET_LENGTH;
use t1_touchbar_hw::wire::{
    self, AncillaryMetadata, ClientMessage, Envelope, ErrorCode, FeatureBits, Hello, HelloAck,
    InputContact, KeyCode, RegisterBuffer, ServiceMessage, StepDirection as WireStepDirection,
    SubmitFrame, TapKeys, WireInputFrame,
};

use crate::desktop_provider::{DesktopAction, DesktopCapabilities, DesktopProvider, DesktopState};
use crate::framebuffer::Xrgb8888Frame;
use crate::gesture::{DisplayContact, GestureEngine, GestureSettings};
use crate::overlay_state::{OverlayState, OverlayWatcher};
use crate::stock_slice::{
    FunctionKey, StepDirection, StockAction, StockBar, StockCapabilities, StockLevels,
};
use crate::touch_id_overlay::TouchIdOverlay;

const BUFFER_ID: u32 = 1;
const FIRST_FRAME_ID: u64 = 1;
const MAX_PENDING_REQUESTS: usize = 64;
const SEND_ATTEMPTS: usize = 64;
const SEND_RETRY_DELAY: Duration = Duration::from_millis(2);
const RECEIVE_RETRY_DELAY: Duration = Duration::from_millis(5);
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_millis(50);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(2);
const VOLUME_STEP_PERCENT: u8 = 6;
const HOLD_REPEAT_DELAY: Duration = Duration::from_millis(350);
const HOLD_REPEAT_INTERVAL: Duration = Duration::from_millis(90);
const OVERLAY_FADE_DURATION: Duration = Duration::from_millis(150);
const ENROLLMENT_PROGRESS_DURATION: Duration = Duration::from_millis(220);

/// Static, redaction-safe failure from one hardware-service connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinRendererError {
    ConnectionUnavailable,
    Transport,
    Protocol,
    Negotiation,
    Frame,
    Gesture,
    ServiceRejected,
}

impl fmt::Display for BuiltinRendererError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConnectionUnavailable => "Touch Bar hardware service is unavailable",
            Self::Transport => "Touch Bar hardware connection was lost",
            Self::Protocol => "Touch Bar hardware protocol failed",
            Self::Negotiation => "Touch Bar display negotiation failed",
            Self::Frame => "Touch Bar frame setup failed",
            Self::Gesture => "Touch Bar input sequence failed",
            Self::ServiceRejected => "Touch Bar hardware request was rejected",
        })
    }
}

impl std::error::Error for BuiltinRendererError {}

/// Runs the built-in renderer and reconnects forever with capped backoff.
pub fn run_forever(provider_path: Option<&Path>) {
    let provider = provider_path.and_then(DesktopProvider::start);
    let mut delay = INITIAL_RECONNECT_DELAY;
    loop {
        if let Err(error) = t1_platform::diagnostics::observe(
            t1_platform::diagnostics::Component::Renderer,
            t1_platform::diagnostics::Stage::RendererConnect,
            || run_once(provider.as_ref()),
        ) {
            eprintln!("t1-touchbar: {error}; reconnecting");
        }
        thread::sleep(delay);
        delay = next_reconnect_delay(delay);
    }
}

fn next_reconnect_delay(delay: Duration) -> Duration {
    delay.saturating_mul(2).min(MAX_RECONNECT_DELAY)
}

fn run_once(provider: Option<&DesktopProvider>) -> Result<(), BuiltinRendererError> {
    let NegotiatedConnection {
        mut connection,
        request_ids,
        ack,
        features,
    } = negotiate_default_features()?;
    let dimensions = DisplayDimensions::new(ack.width, ack.height)
        .map_err(|_| BuiltinRendererError::Negotiation)?;
    let mut session = RendererSession::new(
        dimensions,
        request_ids,
        stock_capabilities(features, ack.minor),
    )?;
    session.register(&connection)?;
    let overlay_watcher = OverlayWatcher::start();
    let clock = Instant::now();

    loop {
        session.advance(clock.elapsed(), &connection, provider)?;
        if let Some(state) = provider.and_then(DesktopProvider::take_latest) {
            session.set_desktop_state(state)?;
            session.submit_if_ready(&connection)?;
        }
        if let Some(observation) = overlay_watcher.take_latest() {
            let overlay = observation.overlay();
            session.set_overlay(
                overlay.map(|overlay| overlay.state),
                overlay.and_then(|overlay| overlay.enrollment_progress),
            );
            session.submit_if_ready(&connection)?;
        }
        if let Some(envelope) = connection.try_receive_service(Some(dimensions))? {
            session.handle_with_provider(envelope, &connection, provider)?;
        } else {
            thread::sleep(RECEIVE_RETRY_DELAY);
        }
    }
}

struct HardwareConnection {
    descriptor: OwnedFd,
    receive_buffer: Vec<u8>,
}

impl HardwareConnection {
    fn new(descriptor: OwnedFd) -> Self {
        Self {
            descriptor,
            receive_buffer: vec![0; MAX_PACKET_LENGTH],
        }
    }

    fn client(&self) -> SeqPacketClient<'_> {
        SeqPacketClient::new(self.descriptor.as_fd())
    }

    fn send_request(
        &self,
        envelope: &Envelope<ClientMessage>,
        dimensions: Option<DisplayDimensions>,
        descriptor: Option<BorrowedFd<'_>>,
    ) -> Result<(), BuiltinRendererError> {
        let packet = wire::encode_client(envelope, dimensions)
            .map_err(|_| BuiltinRendererError::Protocol)?;
        for _ in 0..SEND_ATTEMPTS {
            match self.client().send_with_fd(&packet, descriptor) {
                Ok(()) => return Ok(()),
                Err(SeqPacketError::WouldBlock | SeqPacketError::Interrupted) => {
                    thread::sleep(SEND_RETRY_DELAY);
                }
                Err(_) => return Err(BuiltinRendererError::Transport),
            }
        }
        Err(BuiltinRendererError::Transport)
    }

    fn receive_service(
        &mut self,
        dimensions: Option<DisplayDimensions>,
    ) -> Result<Envelope<ServiceMessage>, BuiltinRendererError> {
        loop {
            let client = SeqPacketClient::new(self.descriptor.as_fd());
            match client.receive(&mut self.receive_buffer) {
                Ok(length) => {
                    return wire::decode_service(
                        &self.receive_buffer[..length],
                        AncillaryMetadata::default(),
                        dimensions,
                    )
                    .map_err(|_| BuiltinRendererError::Protocol);
                }
                Err(SeqPacketError::WouldBlock | SeqPacketError::Interrupted) => {
                    thread::sleep(RECEIVE_RETRY_DELAY);
                }
                Err(_) => return Err(BuiltinRendererError::Transport),
            }
        }
    }

    fn try_receive_service(
        &mut self,
        dimensions: Option<DisplayDimensions>,
    ) -> Result<Option<Envelope<ServiceMessage>>, BuiltinRendererError> {
        let client = SeqPacketClient::new(self.descriptor.as_fd());
        match client.receive(&mut self.receive_buffer) {
            Ok(length) => wire::decode_service(
                &self.receive_buffer[..length],
                AncillaryMetadata::default(),
                dimensions,
            )
            .map(Some)
            .map_err(|_| BuiltinRendererError::Protocol),
            Err(SeqPacketError::WouldBlock | SeqPacketError::Interrupted) => Ok(None),
            Err(_) => Err(BuiltinRendererError::Transport),
        }
    }
}

struct NegotiatedConnection {
    connection: HardwareConnection,
    request_ids: RequestIds,
    ack: HelloAck,
    features: FeatureBits,
}

enum HelloOutcome {
    Accepted(NegotiatedConnection),
    Unsupported,
}

fn negotiate_default_features() -> Result<NegotiatedConnection, BuiltinRendererError> {
    negotiate_default_features_with(|features| {
        Ok(match connect_and_hello(features)? {
            HelloOutcome::Accepted(connection) => Some(connection),
            HelloOutcome::Unsupported => None,
        })
    })?
    .ok_or(BuiltinRendererError::Negotiation)
}

fn negotiate_default_features_with<Connection, Error>(
    mut connect: impl FnMut(FeatureBits) -> Result<Option<Connection>, Error>,
) -> Result<Option<Connection>, Error> {
    let base = FeatureBits::INITIAL_SERVICE | FeatureBits::TOUCH_ID_CANCELLATION;
    let optional = FeatureBits::DISPLAY_BRIGHTNESS | FeatureBits::KEYBOARD_BACKLIGHT;
    if let Some(connection) = connect(base | optional)? {
        return Ok(Some(connection));
    }

    let mut supported = base;
    for feature in [
        FeatureBits::DISPLAY_BRIGHTNESS,
        FeatureBits::KEYBOARD_BACKLIGHT,
    ] {
        if connect(base | feature)?.is_some() {
            supported = supported | feature;
        }
    }
    connect(supported)
}

fn connect_and_hello(features: FeatureBits) -> Result<HelloOutcome, BuiltinRendererError> {
    let descriptor =
        seqpacket::connect_touchbar().map_err(|_| BuiltinRendererError::ConnectionUnavailable)?;
    let mut connection = HardwareConnection::new(descriptor);
    let mut request_ids = RequestIds::new();
    let request_id = request_ids.next(&BTreeMap::new())?;
    connection.send_request(
        &Envelope {
            request_id,
            message: ClientMessage::Hello(Hello {
                minor: wire::PROTOCOL_MINOR,
                required_features: features,
            }),
        },
        None,
        None,
    )?;
    let response = connection.receive_service(None)?;
    if response.request_id != request_id {
        return Err(BuiltinRendererError::Protocol);
    }
    match response.message {
        ServiceMessage::HelloAck(ack) => Ok(HelloOutcome::Accepted(NegotiatedConnection {
            connection,
            request_ids,
            ack,
            features,
        })),
        ServiceMessage::Error(ErrorCode::UnsupportedFeature) => Ok(HelloOutcome::Unsupported),
        ServiceMessage::Error(_) => Err(BuiltinRendererError::ServiceRejected),
        _ => Err(BuiltinRendererError::Protocol),
    }
}

fn stock_capabilities(features: FeatureBits, protocol_minor: u16) -> StockCapabilities {
    let mut capabilities = StockCapabilities::default();
    if protocol_minor >= 1 && features.contains(FeatureBits::DISPLAY_BRIGHTNESS) {
        capabilities = capabilities | StockCapabilities::DISPLAY_BRIGHTNESS;
    }
    if protocol_minor >= 1 && features.contains(FeatureBits::KEYBOARD_BACKLIGHT) {
        capabilities = capabilities | StockCapabilities::KEYBOARD_BACKLIGHT;
    }
    capabilities
}

fn desktop_stock_capabilities(state: DesktopState) -> StockCapabilities {
    let mut capabilities = StockCapabilities::default();
    if state.capabilities.contains(DesktopCapabilities::AUDIO) {
        capabilities = capabilities | StockCapabilities::AUDIO;
    }
    if state.capabilities.contains(DesktopCapabilities::MEDIA) {
        capabilities = capabilities | StockCapabilities::MEDIA;
    }
    capabilities
}

fn dispatch_desktop(provider: Option<&DesktopProvider>, action: DesktopAction) {
    if let Some(provider) = provider {
        let _ = provider.dispatch(action);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingRequest {
    Register,
    Submit { frame_id: u64 },
    TapKeys,
    CancelTouchId,
    HardwareAction(Option<DesktopAction>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InFlightFrame {
    frame_id: u64,
    acknowledged: bool,
}

struct RendererSession {
    dimensions: DisplayDimensions,
    stride: u32,
    byte_length: u64,
    stock: StockBar,
    hardware_capabilities: StockCapabilities,
    desktop_state: DesktopState,
    pending_desktop_state: Option<DesktopState>,
    display_off: bool,
    levels: StockLevels,
    touch_id: TouchIdOverlay,
    overlay_state: Option<OverlayState>,
    overlay_visual_state: Option<OverlayState>,
    overlay_opacity: u8,
    overlay_start_opacity: u8,
    overlay_target_opacity: u8,
    overlay_last_update: Duration,
    enrollment_progress: Option<u8>,
    enrollment_progress_start: u16,
    enrollment_progress_visual: u16,
    enrollment_progress_target: u16,
    enrollment_progress_last_update: Duration,
    now: Duration,
    input: InputInterpreter,
    frame: RendererFrame,
    registered: bool,
    dirty: bool,
    in_flight: Option<InFlightFrame>,
    next_frame_id: u64,
    request_ids: RequestIds,
    pending: BTreeMap<u32, PendingRequest>,
}

impl RendererSession {
    fn new(
        dimensions: DisplayDimensions,
        request_ids: RequestIds,
        capabilities: StockCapabilities,
    ) -> Result<Self, BuiltinRendererError> {
        let stride = dimensions
            .width()
            .checked_mul(4)
            .ok_or(BuiltinRendererError::Negotiation)?;
        let byte_length = u64::from(stride)
            .checked_mul(u64::from(dimensions.height()))
            .ok_or(BuiltinRendererError::Negotiation)?;
        let frame = RendererFrame::new(byte_length).map_err(|_| BuiltinRendererError::Frame)?;
        let touch_id = TouchIdOverlay::new(dimensions.width(), dimensions.height());
        let stock = StockBar::new(dimensions.width(), dimensions.height(), capabilities)
            .map_err(|_| BuiltinRendererError::Negotiation)?;
        let input = InputInterpreter::new(dimensions, stock)?;
        Ok(Self {
            dimensions,
            stride,
            byte_length,
            stock,
            hardware_capabilities: capabilities,
            desktop_state: DesktopState::default(),
            pending_desktop_state: None,
            display_off: false,
            levels: StockLevels::default(),
            touch_id,
            overlay_state: None,
            overlay_visual_state: None,
            overlay_opacity: 0,
            overlay_start_opacity: 0,
            overlay_target_opacity: 0,
            overlay_last_update: Duration::ZERO,
            enrollment_progress: None,
            enrollment_progress_start: 0,
            enrollment_progress_visual: 0,
            enrollment_progress_target: 0,
            enrollment_progress_last_update: Duration::ZERO,
            now: Duration::ZERO,
            input,
            frame,
            registered: false,
            dirty: true,
            in_flight: None,
            next_frame_id: FIRST_FRAME_ID,
            request_ids,
            pending: BTreeMap::new(),
        })
    }

    fn register(&mut self, connection: &HardwareConnection) -> Result<(), BuiltinRendererError> {
        let request_id = self.allocate_request(PendingRequest::Register)?;
        let message = Envelope {
            request_id,
            message: ClientMessage::RegisterBuffer(RegisterBuffer {
                buffer_id: BUFFER_ID,
                stride: self.stride,
                byte_length: self.byte_length,
            }),
        };
        if let Err(error) = connection.send_request(
            &message,
            Some(self.dimensions),
            Some(self.frame.descriptor()),
        ) {
            self.pending.remove(&request_id);
            return Err(error);
        }
        Ok(())
    }

    #[cfg(test)]
    fn handle(
        &mut self,
        envelope: Envelope<ServiceMessage>,
        connection: &HardwareConnection,
    ) -> Result<(), BuiltinRendererError> {
        self.handle_with_provider(envelope, connection, None)
    }

    fn handle_with_provider(
        &mut self,
        envelope: Envelope<ServiceMessage>,
        connection: &HardwareConnection,
        provider: Option<&DesktopProvider>,
    ) -> Result<(), BuiltinRendererError> {
        match envelope.message {
            ServiceMessage::Ack => self.handle_ack(envelope.request_id, connection, provider),
            ServiceMessage::Error(code) => self.handle_error(envelope.request_id, code, connection),
            ServiceMessage::FrameReleased(release) => {
                if envelope.request_id != 0 || release.buffer_id != BUFFER_ID {
                    return Err(BuiltinRendererError::Protocol);
                }
                let Some(in_flight) = self.in_flight else {
                    return Err(BuiltinRendererError::Protocol);
                };
                if !in_flight.acknowledged || release.frame_id != in_flight.frame_id {
                    return Err(BuiltinRendererError::Protocol);
                }
                self.in_flight = None;
                self.submit_if_ready(connection)
            }
            ServiceMessage::InputFrame(frame) => {
                if envelope.request_id != 0 {
                    return Err(BuiltinRendererError::Protocol);
                }
                let outcome = self.observe_input(frame)?;
                self.dirty |= outcome.redraw;
                for action in outcome.actions {
                    self.dispatch(action, connection, provider)?;
                }
                if outcome.cancel_touch_id {
                    self.send_cancel_touch_id(connection)?;
                }
                self.apply_pending_desktop_state()?;
                self.submit_if_ready(connection)
            }
            ServiceMessage::HelloAck(_) => Err(BuiltinRendererError::Protocol),
        }
    }

    fn handle_ack(
        &mut self,
        request_id: u32,
        connection: &HardwareConnection,
        provider: Option<&DesktopProvider>,
    ) -> Result<(), BuiltinRendererError> {
        let pending = self
            .pending
            .remove(&request_id)
            .ok_or(BuiltinRendererError::Protocol)?;
        match pending {
            PendingRequest::Register if !self.registered => {
                self.registered = true;
                self.submit_if_ready(connection)
            }
            PendingRequest::Submit { frame_id } => {
                let Some(in_flight) = self.in_flight.as_mut() else {
                    return Err(BuiltinRendererError::Protocol);
                };
                if in_flight.frame_id != frame_id || in_flight.acknowledged {
                    return Err(BuiltinRendererError::Protocol);
                }
                in_flight.acknowledged = true;
                Ok(())
            }
            PendingRequest::HardwareAction(feedback) => {
                if let Some(feedback) = feedback {
                    dispatch_desktop(provider, feedback);
                }
                Ok(())
            }
            PendingRequest::CancelTouchId => {
                emit(Record::new(
                    Component::Renderer,
                    Stage::TouchIdCancel,
                    Outcome::Ok,
                    None,
                ));
                Ok(())
            }
            PendingRequest::TapKeys => Ok(()),
            PendingRequest::Register => Err(BuiltinRendererError::Protocol),
        }
    }

    fn handle_error(
        &mut self,
        request_id: u32,
        _code: ErrorCode,
        _connection: &HardwareConnection,
    ) -> Result<(), BuiltinRendererError> {
        let pending = self
            .pending
            .remove(&request_id)
            .ok_or(BuiltinRendererError::Protocol)?;
        if matches!(pending, PendingRequest::CancelTouchId) {
            emit(Record::new(
                Component::Renderer,
                Stage::TouchIdCancel,
                Outcome::Error,
                None,
            ));
        }
        match pending {
            PendingRequest::TapKeys
            | PendingRequest::CancelTouchId
            | PendingRequest::HardwareAction(_) => {
                eprintln!("t1-touchbar: typed action was rejected");
                Ok(())
            }
            PendingRequest::Register | PendingRequest::Submit { .. } => {
                Err(BuiltinRendererError::ServiceRejected)
            }
        }
    }

    /// Feeds one hardware input frame through the interpreter.
    ///
    /// While the desktop display is off the panel is dark, so contacts are
    /// dropped before interpretation: nothing is pressed, repeated, tapped, or
    /// used to cancel Touch ID. Fn state is still tracked so the row is right
    /// when the display returns.
    fn observe_input(
        &mut self,
        mut frame: WireInputFrame,
    ) -> Result<InputOutcome, BuiltinRendererError> {
        if self.display_off {
            frame.contacts.clear();
        }
        self.input.ingest_at(&frame, self.overlay_state, self.now)
    }

    fn send_keys(
        &mut self,
        keys: &[KeyCode],
        connection: &HardwareConnection,
    ) -> Result<(), BuiltinRendererError> {
        for keys in keys.chunks(wire::MAX_KEYS) {
            let request_id = self.allocate_request(PendingRequest::TapKeys)?;
            let message = Envelope {
                request_id,
                message: ClientMessage::TapKeys(TapKeys {
                    keys: keys.to_vec(),
                }),
            };
            if let Err(error) = connection.send_request(&message, Some(self.dimensions), None) {
                self.pending.remove(&request_id);
                return Err(error);
            }
        }
        Ok(())
    }

    fn send_cancel_touch_id(
        &mut self,
        connection: &HardwareConnection,
    ) -> Result<(), BuiltinRendererError> {
        let request_id = self.allocate_request(PendingRequest::CancelTouchId)?;
        let message = cancellation_request(request_id);
        emit(Record::new(
            Component::Renderer,
            Stage::TouchIdCancel,
            Outcome::Begin,
            None,
        ));
        if let Err(error) = connection.send_request(&message, Some(self.dimensions), None) {
            emit(Record::new(
                Component::Renderer,
                Stage::TouchIdCancel,
                Outcome::Error,
                None,
            ));
            self.pending.remove(&request_id);
            return Err(error);
        }
        Ok(())
    }

    fn dispatch(
        &mut self,
        action: StockAction,
        connection: &HardwareConnection,
        provider: Option<&DesktopProvider>,
    ) -> Result<(), BuiltinRendererError> {
        if let Some(key) = action_key(action) {
            return self.send_keys(&[key], connection);
        }
        match action {
            StockAction::StepDisplayBrightness(direction) => {
                let feedback = self
                    .desktop_state
                    .capabilities
                    .contains(DesktopCapabilities::LEVEL_FEEDBACK)
                    .then_some(DesktopAction::ShowDisplayBrightness);
                self.send_hardware_action(
                    ClientMessage::StepDisplayBrightness(wire_step(direction)),
                    feedback,
                    connection,
                )
            }
            StockAction::StepKeyboardBacklight(direction) => {
                let feedback = self
                    .desktop_state
                    .capabilities
                    .contains(DesktopCapabilities::LEVEL_FEEDBACK)
                    .then_some(DesktopAction::ShowKeyboardBacklight);
                self.send_hardware_action(
                    ClientMessage::StepKeyboardBacklight(wire_step(direction)),
                    feedback,
                    connection,
                )
            }
            StockAction::ToggleMute => {
                self.levels.muted = !self.levels.muted;
                self.dirty = true;
                dispatch_desktop(provider, DesktopAction::ToggleMute);
                Ok(())
            }
            StockAction::StepVolume(direction) => {
                let current = self.levels.volume.unwrap_or(0);
                let value = match direction {
                    StepDirection::Down => current.saturating_sub(VOLUME_STEP_PERCENT),
                    StepDirection::Up => current.saturating_add(VOLUME_STEP_PERCENT).min(100),
                };
                self.levels.volume = Some(value);
                self.dirty = true;
                dispatch_desktop(provider, DesktopAction::SetVolume(value));
                Ok(())
            }
            StockAction::MediaPrevious => {
                dispatch_desktop(provider, DesktopAction::MediaPrevious);
                Ok(())
            }
            StockAction::MediaPlayPause => {
                dispatch_desktop(provider, DesktopAction::MediaPlayPause);
                Ok(())
            }
            StockAction::MediaNext => {
                dispatch_desktop(provider, DesktopAction::MediaNext);
                Ok(())
            }
            StockAction::Escape | StockAction::Function(_) => unreachable!(),
        }
    }

    fn send_hardware_action(
        &mut self,
        action: ClientMessage,
        feedback: Option<DesktopAction>,
        connection: &HardwareConnection,
    ) -> Result<(), BuiltinRendererError> {
        let request_id = self.allocate_request(PendingRequest::HardwareAction(feedback))?;
        let message = Envelope {
            request_id,
            message: action,
        };
        if let Err(error) = connection.send_request(&message, Some(self.dimensions), None) {
            self.pending.remove(&request_id);
            return Err(error);
        }
        Ok(())
    }

    fn submit_if_ready(
        &mut self,
        connection: &HardwareConnection,
    ) -> Result<(), BuiltinRendererError> {
        if !self.registered || !self.dirty || self.in_flight.is_some() {
            return Ok(());
        }
        self.render()?;
        let frame_id = self.take_frame_id();
        let request_id = self.allocate_request(PendingRequest::Submit { frame_id })?;
        let message = Envelope {
            request_id,
            message: ClientMessage::SubmitFrame(SubmitFrame {
                buffer_id: BUFFER_ID,
                frame_id,
                damage: Vec::new(),
            }),
        };
        if let Err(error) = connection.send_request(&message, Some(self.dimensions), None) {
            self.pending.remove(&request_id);
            return Err(error);
        }
        self.in_flight = Some(InFlightFrame {
            frame_id,
            acknowledged: false,
        });
        self.dirty = false;
        Ok(())
    }

    fn render(&mut self) -> Result<(), BuiltinRendererError> {
        if self.display_off {
            self.frame.as_mut_slice().fill(0);
            return Ok(());
        }
        let mut frame = Xrgb8888Frame::new(
            self.dimensions.width(),
            self.dimensions.height(),
            self.stride,
            self.frame.as_mut_slice(),
        )
        .map_err(|_| BuiltinRendererError::Frame)?;
        let fn_visible = self.input.fn_pressed() && self.overlay_visual_state.is_none();
        self.stock
            .render_pressed(&mut frame, fn_visible, self.levels, self.input.pressed())
            .map_err(|_| BuiltinRendererError::Frame)?;
        if self.overlay_opacity > 0 {
            let left = self.stock.escape_right();
            if left < self.dimensions.width() {
                frame
                    .blend_rectangle(
                        crate::framebuffer::Rectangle {
                            x: left,
                            y: 0,
                            width: self.dimensions.width() - left,
                            height: self.dimensions.height(),
                        },
                        crate::framebuffer::Rgb {
                            red: 0,
                            green: 0,
                            blue: 0,
                        },
                        self.overlay_opacity,
                    )
                    .map_err(|_| BuiltinRendererError::Frame)?;
            }
        }
        self.touch_id
            .render_with_progress(
                &mut frame,
                self.overlay_visual_state,
                (self.overlay_visual_state == Some(OverlayState::Enrollment))
                    .then_some(self.enrollment_progress_visual),
                self.overlay_opacity,
            )
            .map_err(|_| BuiltinRendererError::Frame)
    }

    fn set_overlay(&mut self, state: Option<OverlayState>, progress: Option<u8>) {
        if self.overlay_state == state && self.enrollment_progress == progress {
            return;
        }
        let state_changed = self.overlay_state != state;
        self.overlay_state = state;
        self.enrollment_progress = progress;
        self.enrollment_progress_start = self.enrollment_progress_visual;
        self.enrollment_progress_target = u16::from(progress.unwrap_or(0)) * 10;
        self.enrollment_progress_last_update = self.now;
        if state_changed {
            emit(Record::new(
                Component::Renderer,
                Stage::Overlay,
                if state.is_some() {
                    Outcome::Begin
                } else {
                    Outcome::Ok
                },
                None,
            ));
            self.overlay_start_opacity = self.overlay_opacity;
            self.overlay_last_update = self.now;
            match state {
                Some(state) => {
                    self.input.cancel_repeat();
                    self.overlay_visual_state = Some(state);
                    self.overlay_target_opacity = u8::MAX;
                }
                None => self.overlay_target_opacity = 0,
            }
        }
        self.dirty = true;
    }

    fn advance(
        &mut self,
        now: Duration,
        connection: &HardwareConnection,
        provider: Option<&DesktopProvider>,
    ) -> Result<(), BuiltinRendererError> {
        self.now = now;
        self.advance_overlay_fade(now);
        self.advance_enrollment_progress(now);
        if let Some(action) = self.input.take_repeat(now) {
            self.dispatch(action, connection, provider)?;
        }
        self.submit_if_ready(connection)
    }

    fn advance_overlay_fade(&mut self, now: Duration) {
        if self.overlay_opacity == self.overlay_target_opacity {
            return;
        }
        let elapsed = now.saturating_sub(self.overlay_last_update);
        let duration_nanos = OVERLAY_FADE_DURATION.as_nanos().max(1);
        let progress = elapsed.as_nanos().min(duration_nanos);
        self.overlay_opacity = if self.overlay_target_opacity >= self.overlay_start_opacity {
            let span = self.overlay_target_opacity - self.overlay_start_opacity;
            self.overlay_start_opacity.saturating_add(
                u8::try_from(u128::from(span) * progress / duration_nanos).unwrap_or(span),
            )
        } else {
            let span = self.overlay_start_opacity - self.overlay_target_opacity;
            self.overlay_start_opacity.saturating_sub(
                u8::try_from(u128::from(span) * progress / duration_nanos).unwrap_or(span),
            )
        };
        if self.overlay_opacity == 0 && self.overlay_target_opacity == 0 {
            self.overlay_visual_state = None;
        }
        self.dirty = true;
    }

    fn advance_enrollment_progress(&mut self, now: Duration) {
        if self.enrollment_progress_visual == self.enrollment_progress_target {
            return;
        }
        let elapsed = now.saturating_sub(self.enrollment_progress_last_update);
        let duration_nanos = ENROLLMENT_PROGRESS_DURATION.as_nanos().max(1);
        let progress = elapsed.as_nanos().min(duration_nanos);
        let start = self.enrollment_progress_start;
        let target = self.enrollment_progress_target;
        self.enrollment_progress_visual = if target >= start {
            start.saturating_add(
                u16::try_from(u128::from(target - start) * progress / duration_nanos)
                    .unwrap_or(target - start),
            )
        } else {
            start.saturating_sub(
                u16::try_from(u128::from(start - target) * progress / duration_nanos)
                    .unwrap_or(start - target),
            )
        };
        self.dirty = true;
    }

    fn set_desktop_state(&mut self, state: DesktopState) -> Result<(), BuiltinRendererError> {
        // Display power follows the desktop immediately, even under an active
        // touch: a dark panel must not keep showing controls.
        if self.display_off != state.display_off {
            self.display_off = state.display_off;
            self.dirty = true;
        }
        if self.desktop_state == state && self.pending_desktop_state.is_none() {
            if self.levels.volume != state.volume || self.levels.muted != state.muted {
                self.levels.volume = state.volume;
                self.levels.muted = state.muted;
                self.dirty = true;
            }
            return Ok(());
        }
        if !self.input.is_idle() {
            self.pending_desktop_state = Some(state);
            return Ok(());
        }
        self.apply_desktop_state(state)
    }

    fn apply_pending_desktop_state(&mut self) -> Result<(), BuiltinRendererError> {
        if !self.input.is_idle() {
            return Ok(());
        }
        let Some(state) = self.pending_desktop_state.take() else {
            return Ok(());
        };
        self.apply_desktop_state(state)
    }

    fn apply_desktop_state(&mut self, state: DesktopState) -> Result<(), BuiltinRendererError> {
        let capabilities = self.hardware_capabilities | desktop_stock_capabilities(state);
        let stock = StockBar::new(
            self.dimensions.width(),
            self.dimensions.height(),
            capabilities,
        )
        .map_err(|_| BuiltinRendererError::Negotiation)?;
        self.stock = stock;
        self.input.set_stock(stock);
        self.levels.volume = state.volume;
        self.levels.muted = state.muted;
        self.desktop_state = state;
        self.pending_desktop_state = None;
        self.dirty = true;
        Ok(())
    }

    fn allocate_request(&mut self, kind: PendingRequest) -> Result<u32, BuiltinRendererError> {
        if self.pending.len() >= MAX_PENDING_REQUESTS {
            return Err(BuiltinRendererError::Protocol);
        }
        let request_id = self.request_ids.next(&self.pending)?;
        self.pending.insert(request_id, kind);
        Ok(request_id)
    }

    fn take_frame_id(&mut self) -> u64 {
        let current = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.wrapping_add(1);
        if self.next_frame_id == 0 {
            self.next_frame_id = 1;
        }
        current
    }
}

const fn wire_step(direction: StepDirection) -> WireStepDirection {
    match direction {
        StepDirection::Down => WireStepDirection::Down,
        StepDirection::Up => WireStepDirection::Up,
    }
}

const fn cancellation_request(request_id: u32) -> Envelope<ClientMessage> {
    Envelope {
        request_id,
        message: ClientMessage::CancelTouchId,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RequestIds {
    next: u32,
}

impl RequestIds {
    const fn new() -> Self {
        Self { next: 1 }
    }

    fn next(
        &mut self,
        pending: &BTreeMap<u32, PendingRequest>,
    ) -> Result<u32, BuiltinRendererError> {
        for _ in 0..=MAX_PENDING_REQUESTS {
            let candidate = self.next;
            self.next = self.next.wrapping_add(1);
            if self.next == 0 {
                self.next = 1;
            }
            if candidate != 0 && !pending.contains_key(&candidate) {
                return Ok(candidate);
            }
        }
        Err(BuiltinRendererError::Protocol)
    }
}

#[derive(Debug, Eq, PartialEq)]
struct InputOutcome {
    redraw: bool,
    actions: Vec<StockAction>,
    cancel_touch_id: bool,
}

struct InputInterpreter {
    stock: StockBar,
    touch_id: TouchIdOverlay,
    gestures: GestureEngine,
    fn_pressed: bool,
    pressed: Option<StockAction>,
    repeat: Option<RepeatState>,
    suppress_tap: Option<StockAction>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RepeatState {
    action: StockAction,
    next_at: Duration,
    fired: bool,
}

impl InputInterpreter {
    fn new(dimensions: DisplayDimensions, stock: StockBar) -> Result<Self, BuiltinRendererError> {
        let touch_id = TouchIdOverlay::new(dimensions.width(), dimensions.height());
        let drag_distance = (f64::from(dimensions.height()) / 4.0).max(1.0);
        let settings = GestureSettings::new(drag_distance, Duration::MAX, wire::MAX_CONTACTS)
            .map_err(|_| BuiltinRendererError::Gesture)?;
        Ok(Self {
            stock,
            touch_id,
            gestures: GestureEngine::new(settings),
            fn_pressed: false,
            pressed: None,
            repeat: None,
            suppress_tap: None,
        })
    }

    const fn fn_pressed(&self) -> bool {
        self.fn_pressed
    }

    const fn pressed(&self) -> Option<StockAction> {
        self.pressed
    }

    fn is_idle(&self) -> bool {
        self.gestures.is_idle()
    }

    fn set_stock(&mut self, stock: StockBar) {
        debug_assert!(self.is_idle());
        self.stock = stock;
    }

    #[cfg(test)]
    fn ingest(
        &mut self,
        frame: &WireInputFrame,
        overlay_state: Option<OverlayState>,
    ) -> Result<InputOutcome, BuiltinRendererError> {
        self.ingest_at(
            frame,
            overlay_state,
            Duration::from_nanos(frame.monotonic_ns),
        )
    }

    fn ingest_at(
        &mut self,
        frame: &WireInputFrame,
        overlay_state: Option<OverlayState>,
        now: Duration,
    ) -> Result<InputOutcome, BuiltinRendererError> {
        let mut redraw = self.fn_pressed != frame.fn_pressed;
        self.fn_pressed = frame.fn_pressed;
        let contacts = frame
            .contacts
            .iter()
            .filter(|contact| contact.tip)
            .map(display_contact)
            .collect::<Result<Vec<_>, _>>()?;
        let pressed = contacts
            .first()
            .filter(|contact| {
                !self
                    .touch_id
                    .covers_contact(**contact, overlay_state, self.fn_pressed)
            })
            .and_then(|contact| self.stock.action_at(*contact, self.fn_pressed));
        redraw |= self.pressed != pressed;
        if self.pressed != pressed {
            if pressed.is_none()
                && let Some(repeat) = self.repeat
                && repeat.fired
            {
                self.suppress_tap = Some(repeat.action);
            }
            self.repeat = pressed
                .filter(|action| repeatable(*action))
                .map(|action| RepeatState {
                    action,
                    next_at: now.saturating_add(HOLD_REPEAT_DELAY),
                    fired: false,
                });
        }
        self.pressed = pressed;
        let gestures = self
            .gestures
            .ingest_frame(Duration::from_nanos(frame.monotonic_ns), &contacts)
            .map_err(|_| BuiltinRendererError::Gesture)?;
        let mut actions = Vec::new();
        let mut cancel_touch_id = false;
        for gesture in gestures {
            if matches!(
                gesture,
                crate::gesture::Gesture::DragStart(_) | crate::gesture::Gesture::Drag { .. }
            ) {
                self.repeat = None;
                self.suppress_tap = None;
            }
            let cancels =
                self.touch_id
                    .cancellation_for_gesture(gesture, overlay_state, self.fn_pressed);
            if overlay_state.is_some() && matches!(gesture, crate::gesture::Gesture::Press(_)) {
                emit(Record::new(
                    Component::Renderer,
                    Stage::OverlayPress,
                    if cancels { Outcome::Ok } else { Outcome::Error },
                    None,
                ));
            }
            if cancels {
                cancel_touch_id = true;
                continue;
            }
            if let crate::gesture::Gesture::Tap(contact) = gesture
                && self
                    .touch_id
                    .covers_contact(contact, overlay_state, self.fn_pressed)
            {
                continue;
            }
            let Some(action) = self.stock.action_for_gesture(gesture, self.fn_pressed) else {
                continue;
            };
            if self.suppress_tap == Some(action) {
                self.suppress_tap = None;
                continue;
            }
            if !actions.contains(&action) {
                actions.push(action);
            }
        }
        Ok(InputOutcome {
            redraw,
            actions,
            cancel_touch_id,
        })
    }

    fn take_repeat(&mut self, now: Duration) -> Option<StockAction> {
        let repeat = self.repeat.as_mut()?;
        if now < repeat.next_at || self.pressed != Some(repeat.action) {
            return None;
        }
        repeat.fired = true;
        repeat.next_at = now.saturating_add(HOLD_REPEAT_INTERVAL);
        Some(repeat.action)
    }

    fn cancel_repeat(&mut self) {
        self.repeat = None;
        self.suppress_tap = None;
    }
}

const fn repeatable(action: StockAction) -> bool {
    matches!(
        action,
        StockAction::StepDisplayBrightness(_)
            | StockAction::StepKeyboardBacklight(_)
            | StockAction::StepVolume(_)
    )
}

fn display_contact(contact: &InputContact) -> Result<DisplayContact, BuiltinRendererError> {
    DisplayContact::new(
        u32::from(contact.id),
        f64::from(contact.x),
        f64::from(contact.y),
    )
    .map_err(|_| BuiltinRendererError::Gesture)
}

const fn action_key(action: StockAction) -> Option<KeyCode> {
    Some(match action {
        StockAction::Escape => KeyCode::Escape,
        StockAction::Function(FunctionKey::F1) => KeyCode::F1,
        StockAction::Function(FunctionKey::F2) => KeyCode::F2,
        StockAction::Function(FunctionKey::F3) => KeyCode::F3,
        StockAction::Function(FunctionKey::F4) => KeyCode::F4,
        StockAction::Function(FunctionKey::F5) => KeyCode::F5,
        StockAction::Function(FunctionKey::F6) => KeyCode::F6,
        StockAction::Function(FunctionKey::F7) => KeyCode::F7,
        StockAction::Function(FunctionKey::F8) => KeyCode::F8,
        StockAction::Function(FunctionKey::F9) => KeyCode::F9,
        StockAction::Function(FunctionKey::F10) => KeyCode::F10,
        StockAction::Function(FunctionKey::F11) => KeyCode::F11,
        StockAction::Function(FunctionKey::F12) => KeyCode::F12,
        StockAction::StepDisplayBrightness(_)
        | StockAction::StepKeyboardBacklight(_)
        | StockAction::ToggleMute
        | StockAction::StepVolume(_)
        | StockAction::MediaPrevious
        | StockAction::MediaPlayPause
        | StockAction::MediaNext => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn dimensions() -> DisplayDimensions {
        DisplayDimensions::new(130, 20).expect("valid synthetic dimensions")
    }

    fn session() -> RendererSession {
        RendererSession::new(
            dimensions(),
            RequestIds { next: 2 },
            StockCapabilities::default(),
        )
        .expect("renderer session")
    }

    fn new_interpreter() -> InputInterpreter {
        let dimensions = dimensions();
        let stock = StockBar::new(
            dimensions.width(),
            dimensions.height(),
            StockCapabilities::default(),
        )
        .unwrap();
        InputInterpreter::new(dimensions, stock).expect("input interpreter")
    }

    fn inert_connection() -> HardwareConnection {
        let (connection, _peer) = UnixStream::pair().expect("synthetic descriptor pair");
        HardwareConnection::new(OwnedFd::from(connection))
    }

    fn input(monotonic_ns: u64, fn_pressed: bool, contacts: Vec<InputContact>) -> WireInputFrame {
        WireInputFrame {
            monotonic_ns,
            fn_pressed,
            contacts,
        }
    }

    fn contact(id: u8, x: u32, y: u32) -> InputContact {
        InputContact {
            id,
            tip: true,
            in_range: true,
            x,
            y,
        }
    }

    #[test]
    fn hello_requires_render_input_keys_and_touch_id_cancellation() {
        let encoded = wire::encode_client(
            &Envelope {
                request_id: 1,
                message: ClientMessage::Hello(Hello {
                    minor: wire::PROTOCOL_MINOR,
                    required_features: FeatureBits::INITIAL_SERVICE
                        | FeatureBits::TOUCH_ID_CANCELLATION,
                }),
            },
            None,
        )
        .expect("encode hello");
        let decoded = wire::decode_client(&encoded, AncillaryMetadata::default(), None)
            .expect("decode hello");
        let ClientMessage::Hello(hello) = decoded.message else {
            panic!("hello message expected");
        };
        assert_eq!(hello.required_features.bits(), 0x27);
    }

    #[test]
    fn feature_negotiation_keeps_the_first_complete_connection() {
        let base = FeatureBits::INITIAL_SERVICE | FeatureBits::TOUCH_ID_CANCELLATION;
        let all = base | FeatureBits::DISPLAY_BRIGHTNESS | FeatureBits::KEYBOARD_BACKLIGHT;
        let mut calls = Vec::new();

        let accepted = negotiate_default_features_with(|required| {
            calls.push(required);
            Ok::<_, ()>(Some(required))
        })
        .expect("feature probe succeeds");

        assert_eq!(accepted, Some(all));
        assert_eq!(calls, [all]);
    }

    #[test]
    fn feature_negotiation_discovers_optional_controls_independently() {
        let base = FeatureBits::INITIAL_SERVICE | FeatureBits::TOUCH_ID_CANCELLATION;
        let display = FeatureBits::DISPLAY_BRIGHTNESS;
        let keyboard = FeatureBits::KEYBOARD_BACKLIGHT;
        let supported = base | display;
        let mut calls = Vec::new();

        let accepted = negotiate_default_features_with(|required| {
            calls.push(required);
            Ok::<_, ()>(supported.contains(required).then_some(required))
        })
        .expect("feature probes succeed");

        assert_eq!(accepted, Some(supported));
        assert_eq!(
            calls,
            [
                base | display | keyboard,
                base | display,
                base | keyboard,
                supported
            ]
        );
    }

    #[test]
    fn feature_negotiation_rejects_a_service_missing_the_base_contract() {
        let base = FeatureBits::INITIAL_SERVICE | FeatureBits::TOUCH_ID_CANCELLATION;
        let display = FeatureBits::DISPLAY_BRIGHTNESS;
        let keyboard = FeatureBits::KEYBOARD_BACKLIGHT;
        let mut calls = Vec::new();

        let accepted = negotiate_default_features_with(|required| {
            calls.push(required);
            Ok::<_, ()>(None::<FeatureBits>)
        })
        .expect("unsupported probes are valid responses");

        assert_eq!(accepted, None);
        assert_eq!(
            calls,
            [
                base | display | keyboard,
                base | display,
                base | keyboard,
                base
            ]
        );
    }

    #[test]
    fn negotiated_root_features_enable_only_matching_surface_controls() {
        let base = FeatureBits::INITIAL_SERVICE | FeatureBits::TOUCH_ID_CANCELLATION;
        assert_eq!(stock_capabilities(base, 1), StockCapabilities::default());
        assert_eq!(
            stock_capabilities(base | FeatureBits::DISPLAY_BRIGHTNESS, 1),
            StockCapabilities::DISPLAY_BRIGHTNESS
        );
        assert_eq!(
            stock_capabilities(base | FeatureBits::KEYBOARD_BACKLIGHT, 1),
            StockCapabilities::KEYBOARD_BACKLIGHT
        );
        assert_eq!(
            stock_capabilities(
                base | FeatureBits::DISPLAY_BRIGHTNESS | FeatureBits::KEYBOARD_BACKLIGHT,
                0
            ),
            StockCapabilities::default()
        );
    }

    #[test]
    fn provider_state_changes_only_provider_controls_and_levels() {
        let mut session = RendererSession::new(
            dimensions(),
            RequestIds { next: 2 },
            StockCapabilities::DISPLAY_BRIGHTNESS,
        )
        .expect("renderer session");
        let state = DesktopState {
            capabilities: DesktopCapabilities::AUDIO | DesktopCapabilities::MEDIA,
            volume: Some(61),
            muted: true,
            display_off: false,
        };
        session
            .set_desktop_state(state)
            .expect("apply desktop state");
        assert_eq!(session.desktop_state, state);
        assert_eq!(session.levels.volume, Some(61));
        assert!(session.levels.muted);
        assert_eq!(
            session.stock,
            StockBar::new(
                dimensions().width(),
                dimensions().height(),
                StockCapabilities::DISPLAY_BRIGHTNESS
                    | StockCapabilities::AUDIO
                    | StockCapabilities::MEDIA,
            )
            .unwrap()
        );

        session
            .set_desktop_state(DesktopState::default())
            .expect("withdraw desktop state");
        assert_eq!(session.levels.volume, None);
        assert!(!session.levels.muted);
        assert_eq!(
            session.stock,
            StockBar::new(
                dimensions().width(),
                dimensions().height(),
                StockCapabilities::DISPLAY_BRIGHTNESS,
            )
            .unwrap()
        );
    }

    #[test]
    fn unchanged_provider_state_corrects_optimistic_audio_levels() {
        let mut session = session();
        let state = DesktopState {
            capabilities: DesktopCapabilities::AUDIO,
            volume: Some(42),
            muted: false,
            display_off: false,
        };
        session
            .set_desktop_state(state)
            .expect("apply authoritative audio state");
        session.levels.volume = Some(87);
        session.levels.muted = true;
        session.dirty = false;

        session
            .set_desktop_state(state)
            .expect("reconcile unchanged authoritative state");

        assert_eq!(session.desktop_state, state);
        assert_eq!(session.levels.volume, Some(42));
        assert!(!session.levels.muted);
        assert!(session.dirty);
    }

    #[test]
    fn provider_layout_change_waits_for_active_touch_to_end() {
        let mut session = session();
        session
            .input
            .ingest(&input(1, false, vec![contact(1, 60, 10)]), None)
            .expect("start active touch");
        let state = DesktopState {
            capabilities: DesktopCapabilities::AUDIO,
            volume: Some(50),
            muted: false,
            display_off: false,
        };
        session
            .set_desktop_state(state)
            .expect("defer desktop state");
        assert_eq!(session.desktop_state, DesktopState::default());
        assert_eq!(session.pending_desktop_state, Some(state));

        session
            .input
            .ingest(&input(2, false, Vec::new()), None)
            .expect("finish active touch");
        session
            .apply_pending_desktop_state()
            .expect("apply deferred desktop state");
        assert_eq!(session.desktop_state, state);
        assert_eq!(session.pending_desktop_state, None);
    }

    #[test]
    fn display_off_blanks_the_frame_and_drops_contacts_until_power_returns() {
        let mut session = session();
        session.render().expect("render stock frame");
        let stock = session.frame.as_mut_slice().to_vec();
        session.dirty = false;

        let off = DesktopState {
            capabilities: DesktopCapabilities::DISPLAY_POWER,
            volume: None,
            muted: false,
            display_off: true,
        };
        session.set_desktop_state(off).expect("apply display off");
        assert!(session.display_off);
        assert!(session.dirty);
        session.render().expect("render dark frame");
        assert!(session.frame.as_mut_slice().iter().all(|byte| *byte == 0));

        let outcome = session
            .observe_input(input(1, false, vec![contact(1, 5, 10)]))
            .expect("observe contact on dark panel");
        assert!(outcome.actions.is_empty());
        assert!(!outcome.cancel_touch_id);
        assert_eq!(session.input.pressed(), None);
        let outcome = session
            .observe_input(input(2, false, Vec::new()))
            .expect("observe release on dark panel");
        assert!(outcome.actions.is_empty());

        let on = DesktopState {
            display_off: false,
            ..off
        };
        session.dirty = false;
        session.set_desktop_state(on).expect("apply display on");
        assert!(!session.display_off);
        assert!(session.dirty);
        session.render().expect("render restored frame");
        assert_eq!(session.frame.as_mut_slice(), stock);
        session
            .observe_input(input(3, false, vec![contact(1, 5, 10)]))
            .expect("observe contact on lit panel");
        assert_eq!(session.input.pressed(), Some(StockAction::Escape));
    }

    #[test]
    fn display_power_applies_under_an_active_touch_while_layout_waits() {
        let mut session = session();
        session
            .input
            .ingest(&input(1, false, vec![contact(1, 60, 10)]), None)
            .expect("start active touch");
        let state = DesktopState {
            capabilities: DesktopCapabilities::AUDIO | DesktopCapabilities::DISPLAY_POWER,
            volume: Some(50),
            muted: false,
            display_off: true,
        };
        session
            .set_desktop_state(state)
            .expect("defer layout but not display power");
        assert!(session.display_off);
        assert_eq!(session.desktop_state, DesktopState::default());
        assert_eq!(session.pending_desktop_state, Some(state));
        session.render().expect("render dark frame under touch");
        assert!(session.frame.as_mut_slice().iter().all(|byte| *byte == 0));
    }

    #[test]
    fn brightness_steps_map_to_typed_wire_directions() {
        assert_eq!(wire_step(StepDirection::Down), WireStepDirection::Down);
        assert_eq!(wire_step(StepDirection::Up), WireStepDirection::Up);
    }

    #[test]
    fn reconnect_backoff_doubles_and_stays_capped() {
        let mut delay = INITIAL_RECONNECT_DELAY;
        let expected = [100, 200, 400, 800, 1_600, 2_000, 2_000];
        for expected_millis in expected {
            delay = next_reconnect_delay(delay);
            assert_eq!(delay, Duration::from_millis(expected_millis));
        }
    }

    #[test]
    fn production_session_renders_initial_escape_and_fn_redraw() {
        let mut session = session();
        assert_eq!(session.stride, 520);
        assert_eq!(session.byte_length, 10_400);
        assert_eq!(session.frame.as_mut_slice().len(), 10_400);
        session.render().expect("render initial Escape frame");
        let initial = session.frame.as_mut_slice().to_vec();
        assert!(initial.iter().any(|byte| *byte != 0));

        session
            .input
            .ingest(&input(1, true, Vec::new()), None)
            .expect("accept Fn transition");
        session.render().expect("render function row");
        assert_ne!(session.frame.as_mut_slice(), initial);
    }

    #[test]
    fn cosmetic_overlay_transitions_dirty_and_change_the_frame_only_on_state_change() {
        let mut session = session();
        session.render().expect("render stock frame");
        let stock = session.frame.as_mut_slice().to_vec();
        session.dirty = false;

        session.set_overlay(Some(OverlayState::Enrollment), Some(0));
        assert!(session.dirty);
        session.advance_overlay_fade(OVERLAY_FADE_DURATION);
        session.render().expect("render enrollment overlay");
        let enrollment = session.frame.as_mut_slice().to_vec();
        assert_ne!(enrollment, stock);

        session.dirty = false;
        session.set_overlay(Some(OverlayState::Enrollment), Some(0));
        assert!(!session.dirty);
        session.set_overlay(Some(OverlayState::Success), None);
        assert!(session.dirty);
        session.render().expect("render success overlay");
        assert_ne!(session.frame.as_mut_slice(), enrollment);

        session.dirty = false;
        session.now = OVERLAY_FADE_DURATION;
        session.set_overlay(None, None);
        assert!(session.dirty);
        session.advance_overlay_fade(OVERLAY_FADE_DURATION.saturating_mul(2));
        session.render().expect("remove cosmetic overlay");
        assert_eq!(session.frame.as_mut_slice(), stock);
    }

    #[test]
    fn enrollment_progress_animates_to_each_bounded_target() {
        let mut session = session();
        session.set_overlay(Some(OverlayState::Enrollment), Some(0));
        session.advance_overlay_fade(OVERLAY_FADE_DURATION);
        session.render().expect("render empty progress");
        let empty = session.frame.as_mut_slice().to_vec();

        session.dirty = false;
        session.now = OVERLAY_FADE_DURATION;
        session.set_overlay(Some(OverlayState::Enrollment), Some(60));
        assert!(session.dirty);
        session
            .advance_enrollment_progress(OVERLAY_FADE_DURATION + ENROLLMENT_PROGRESS_DURATION / 2);
        assert!((1..600).contains(&session.enrollment_progress_visual));
        session.render().expect("render partial progress");
        assert_ne!(session.frame.as_mut_slice(), empty);

        session.advance_enrollment_progress(OVERLAY_FADE_DURATION + ENROLLMENT_PROGRESS_DURATION);
        assert_eq!(session.enrollment_progress_visual, 600);
    }

    #[test]
    fn overlay_crossfade_cancels_hidden_control_repeats() {
        let mut session = RendererSession::new(
            dimensions(),
            RequestIds { next: 2 },
            StockCapabilities::DISPLAY_BRIGHTNESS,
        )
        .expect("renderer session");
        session.input.repeat = Some(RepeatState {
            action: StockAction::StepDisplayBrightness(StepDirection::Down),
            next_at: HOLD_REPEAT_DELAY,
            fired: false,
        });
        session.input.pressed = Some(StockAction::StepDisplayBrightness(StepDirection::Down));

        session.set_overlay(Some(OverlayState::Authenticate), None);
        assert_eq!(session.input.repeat, None);
        session.advance_overlay_fade(OVERLAY_FADE_DURATION / 2);
        assert!(session.overlay_opacity > 0 && session.overlay_opacity < u8::MAX);
        session.advance_overlay_fade(OVERLAY_FADE_DURATION);
        assert_eq!(session.overlay_opacity, u8::MAX);

        session.render().expect("render opaque overlay");
        let hidden_control_offset = 20 * 4;
        assert_eq!(
            &session.frame.as_mut_slice()[hidden_control_offset..hidden_control_offset + 4],
            &[0, 0, 0, 0]
        );

        session.now = OVERLAY_FADE_DURATION;
        session.set_overlay(None, None);
        session.advance_overlay_fade(OVERLAY_FADE_DURATION + OVERLAY_FADE_DURATION / 2);
        assert!(session.overlay_opacity > 0 && session.overlay_opacity < u8::MAX);
        session.advance_overlay_fade(OVERLAY_FADE_DURATION.saturating_mul(2));
        assert_eq!(session.overlay_opacity, 0);
        assert_eq!(session.overlay_visual_state, None);
    }

    #[test]
    fn input_socket_routes_one_cancel_and_consumes_ack_without_release_repeat() {
        let (renderer, hardware) =
            t1_platform::seqpacket::pair_for_test().expect("local hardware protocol socket pair");
        let mut connection = HardwareConnection::new(renderer);
        let hardware = SeqPacketClient::new(hardware.as_fd());
        let mut session = session();
        session.set_overlay(Some(OverlayState::Authenticate), None);
        let mut packet = vec![0; MAX_PACKET_LENGTH];

        for (timestamp, contacts) in [
            (1, vec![contact(1, 60, 10)]),
            (2, vec![contact(1, 60, 10)]),
            (3, Vec::new()),
        ] {
            let frame = wire::encode_service(
                &Envelope {
                    request_id: 0,
                    message: ServiceMessage::InputFrame(input(timestamp, false, contacts)),
                },
                Some(dimensions()),
            )
            .expect("encode input frame");
            hardware.send(&frame).expect("deliver input frame");
            let frame = connection
                .receive_service(Some(dimensions()))
                .expect("receive input frame");
            session.handle(frame, &connection).expect("dispatch input");

            if timestamp != 1 {
                assert_eq!(
                    hardware.receive(&mut packet),
                    Err(SeqPacketError::WouldBlock)
                );
                assert!(session.pending.is_empty());
                continue;
            }
            let length = hardware.receive(&mut packet).expect("receive cancellation");
            let request = wire::decode_client(
                &packet[..length],
                AncillaryMetadata::default(),
                Some(dimensions()),
            )
            .expect("decode cancellation");
            assert!(matches!(request.message, ClientMessage::CancelTouchId));
            assert_eq!(session.pending.len(), 1);
            let ack = wire::encode_service(
                &Envelope {
                    request_id: request.request_id,
                    message: ServiceMessage::Ack,
                },
                Some(dimensions()),
            )
            .expect("encode acknowledgement");
            hardware.send(&ack).expect("deliver acknowledgement");
            let ack = connection
                .receive_service(Some(dimensions()))
                .expect("receive acknowledgement");
            session
                .handle(ack, &connection)
                .expect("consume acknowledgement");
            assert!(session.pending.is_empty());
            assert_eq!(session.overlay_state, Some(OverlayState::Authenticate));
        }
    }

    #[test]
    fn cancellation_request_is_typed_and_ack_is_cosmetic_neutral() {
        let mut session = session();
        session.set_overlay(Some(OverlayState::Authenticate), None);
        let request = cancellation_request(71);
        let packet =
            wire::encode_client(&request, Some(dimensions())).expect("encode typed cancellation");
        let request =
            wire::decode_client(&packet, AncillaryMetadata::default(), Some(dimensions()))
                .expect("decode typed cancellation");
        assert!(matches!(request.message, ClientMessage::CancelTouchId));

        let connection = inert_connection();
        session
            .pending
            .insert(request.request_id, PendingRequest::CancelTouchId);
        session
            .handle(
                Envelope {
                    request_id: request.request_id,
                    message: ServiceMessage::Ack,
                },
                &connection,
            )
            .expect("cancellation delivery acknowledgement");
        assert_eq!(session.overlay_state, Some(OverlayState::Authenticate));
        assert!(!session.pending.contains_key(&request.request_id));
    }

    #[test]
    fn submit_ack_release_and_typed_error_complete_owned_state() {
        let mut session = session();
        let connection = inert_connection();
        session.pending.insert(
            7,
            PendingRequest::Submit {
                frame_id: FIRST_FRAME_ID,
            },
        );
        session.in_flight = Some(InFlightFrame {
            frame_id: FIRST_FRAME_ID,
            acknowledged: false,
        });
        session.dirty = false;

        session
            .handle(
                Envelope {
                    request_id: 7,
                    message: ServiceMessage::Ack,
                },
                &connection,
            )
            .expect("accept matching submit Ack");
        assert_eq!(
            session.in_flight,
            Some(InFlightFrame {
                frame_id: FIRST_FRAME_ID,
                acknowledged: true,
            })
        );
        session
            .handle(
                Envelope {
                    request_id: 0,
                    message: ServiceMessage::FrameReleased(wire::FrameReleased {
                        buffer_id: BUFFER_ID,
                        frame_id: FIRST_FRAME_ID,
                    }),
                },
                &connection,
            )
            .expect("accept matching frame release");
        assert_eq!(session.in_flight, None);

        session.pending.insert(8, PendingRequest::TapKeys);
        session
            .handle(
                Envelope {
                    request_id: 8,
                    message: ServiceMessage::Error(ErrorCode::ActionDenied),
                },
                &connection,
            )
            .expect("consume typed key rejection without reconnecting");
        assert!(!session.pending.contains_key(&8));

        session.overlay_state = Some(OverlayState::Retry);
        session.pending.insert(10, PendingRequest::CancelTouchId);
        session
            .handle(
                Envelope {
                    request_id: 10,
                    message: ServiceMessage::Error(ErrorCode::ActionDenied),
                },
                &connection,
            )
            .expect("consume typed cancellation rejection without reconnecting");
        assert!(!session.pending.contains_key(&10));
        assert_eq!(session.overlay_state, Some(OverlayState::Retry));

        session.pending.insert(9, PendingRequest::Register);
        assert_eq!(
            session.handle(
                Envelope {
                    request_id: 9,
                    message: ServiceMessage::Error(ErrorCode::InvalidBuffer),
                },
                &connection,
            ),
            Err(BuiltinRendererError::ServiceRejected)
        );
    }

    #[test]
    fn mismatched_release_is_a_hard_protocol_failure() {
        let mut session = session();
        let connection = inert_connection();
        session.in_flight = Some(InFlightFrame {
            frame_id: 4,
            acknowledged: true,
        });

        assert_eq!(
            session.handle(
                Envelope {
                    request_id: 0,
                    message: ServiceMessage::FrameReleased(wire::FrameReleased {
                        buffer_id: BUFFER_ID,
                        frame_id: 5,
                    }),
                },
                &connection,
            ),
            Err(BuiltinRendererError::Protocol)
        );
    }

    #[test]
    fn fn_transition_redraws_and_completed_tap_maps_to_typed_function_key() {
        let mut interpreter = new_interpreter();
        assert_eq!(
            interpreter
                .ingest(&input(1, true, Vec::new()), None)
                .expect("Fn transition"),
            InputOutcome {
                redraw: true,
                actions: Vec::new(),
                cancel_touch_id: false,
            }
        );
        assert!(interpreter.fn_pressed());
        assert_eq!(
            interpreter
                .ingest(&input(2, true, vec![contact(4, 15, 8)]), None)
                .expect("function press")
                .actions,
            []
        );
        assert_eq!(
            interpreter
                .ingest(&input(3, true, Vec::new()), None)
                .expect("function release")
                .actions,
            [StockAction::Function(FunctionKey::F1)]
        );
    }

    #[test]
    fn escape_remains_active_without_fn_and_function_regions_do_not() {
        let mut interpreter = new_interpreter();
        interpreter
            .ingest(&input(1, false, vec![contact(1, 4, 8)]), None)
            .expect("Escape press");
        assert_eq!(
            interpreter
                .ingest(&input(2, false, Vec::new()), None)
                .expect("Escape release")
                .actions,
            [StockAction::Escape]
        );
        interpreter
            .ingest(&input(3, false, vec![contact(2, 15, 8)]), None)
            .expect("inactive function press");
        assert!(
            interpreter
                .ingest(&input(4, false, Vec::new()), None)
                .expect("inactive function release")
                .actions
                .is_empty()
        );
    }

    #[test]
    fn drag_does_not_emit_a_key_and_duplicate_slot_taps_are_deduplicated() {
        let mut interpreter = new_interpreter();
        interpreter
            .ingest(&input(1, true, vec![contact(1, 15, 8)]), None)
            .expect("Fn press");
        interpreter
            .ingest(&input(2, true, vec![contact(1, 80, 8)]), None)
            .expect("drag");
        assert!(
            interpreter
                .ingest(&input(3, true, Vec::new()), None)
                .expect("drag release")
                .actions
                .is_empty()
        );

        interpreter
            .ingest(
                &input(4, true, vec![contact(2, 15, 8), contact(3, 16, 8)]),
                None,
            )
            .expect("two presses");
        assert_eq!(
            interpreter
                .ingest(&input(5, true, Vec::new()), None)
                .expect("two releases")
                .actions,
            [StockAction::Function(FunctionKey::F1)]
        );
    }

    #[test]
    fn cancellation_on_press_deduplicates_contacts_and_never_repeats_on_release() {
        let active = Some(OverlayState::Authenticate);
        let mut interpreter = new_interpreter();
        let outcome = interpreter
            .ingest(
                &input(1, false, vec![contact(1, 24, 10), contact(2, 25, 10)]),
                active,
            )
            .expect("two overlay presses");
        assert!(outcome.cancel_touch_id);
        assert!(outcome.actions.is_empty());
        let outcome = interpreter
            .ingest(&input(2, false, Vec::new()), active)
            .expect("two overlay releases");
        assert!(!outcome.cancel_touch_id);
        assert!(outcome.actions.is_empty());

        let mut absent = new_interpreter();
        absent
            .ingest(&input(1, false, vec![contact(1, 24, 10)]), None)
            .expect("press without overlay");
        assert!(
            !absent
                .ingest(&input(2, false, Vec::new()), None)
                .expect("release without overlay")
                .cancel_touch_id
        );

        let mut label = new_interpreter();
        assert!(
            label
                .ingest(&input(1, false, vec![contact(1, 60, 10)]), active)
                .expect("label press")
                .cancel_touch_id
        );
        assert!(
            !label
                .ingest(&input(2, false, Vec::new()), active)
                .expect("label release")
                .cancel_touch_id
        );

        let mut dragged = new_interpreter();
        assert!(
            dragged
                .ingest(&input(1, false, vec![contact(1, 24, 10)]), active)
                .expect("overlay press")
                .cancel_touch_id
        );
        dragged
            .ingest(&input(2, false, vec![contact(1, 80, 10)]), active)
            .expect("overlay drag");
        assert!(
            !dragged
                .ingest(&input(3, false, Vec::new()), active)
                .expect("drag release")
                .cancel_touch_id
        );

        let mut function_row = new_interpreter();
        assert!(
            function_row
                .ingest(&input(1, true, vec![contact(1, 80, 8)]), active)
                .expect("function press")
                .cancel_touch_id
        );
        let outcome = function_row
            .ingest(&input(2, true, Vec::new()), active)
            .expect("function release");
        assert!(!outcome.cancel_touch_id);
        assert!(outcome.actions.is_empty());
    }

    #[test]
    fn held_contact_cannot_cancel_a_new_overlay_and_escape_stays_available() {
        let mut interpreter = new_interpreter();
        let held = vec![contact(1, 80, 10)];
        assert!(
            interpreter
                .ingest(
                    &input(1, false, held.clone()),
                    Some(OverlayState::Authenticate)
                )
                .unwrap()
                .cancel_touch_id
        );
        assert!(
            !interpreter
                .ingest(&input(2, false, held.clone()), None)
                .unwrap()
                .cancel_touch_id
        );
        assert!(
            !interpreter
                .ingest(&input(3, false, held), Some(OverlayState::Approve))
                .unwrap()
                .cancel_touch_id
        );
        assert!(
            !interpreter
                .ingest(&input(4, false, Vec::new()), Some(OverlayState::Approve))
                .unwrap()
                .cancel_touch_id
        );

        let outcome = interpreter
            .ingest(
                &input(5, false, vec![contact(2, 4, 10)]),
                Some(OverlayState::Authenticate),
            )
            .unwrap();
        assert!(!outcome.cancel_touch_id);
        let outcome = interpreter
            .ingest(
                &input(6, false, Vec::new()),
                Some(OverlayState::Authenticate),
            )
            .unwrap();
        assert!(!outcome.cancel_touch_id);
        assert_eq!(outcome.actions, [StockAction::Escape]);
    }

    #[test]
    fn range_controls_repeat_after_hold_and_do_not_add_a_release_step() {
        let dimensions = dimensions();
        let stock = StockBar::new(
            dimensions.width(),
            dimensions.height(),
            StockCapabilities::DISPLAY_BRIGHTNESS,
        )
        .unwrap();
        let mut interpreter = InputInterpreter::new(dimensions, stock).unwrap();
        let held = contact(1, 13, 8);

        interpreter
            .ingest_at(&input(1, false, vec![held]), None, Duration::ZERO)
            .unwrap();
        assert_eq!(
            interpreter.take_repeat(HOLD_REPEAT_DELAY),
            Some(StockAction::StepDisplayBrightness(StepDirection::Down))
        );
        assert_eq!(
            interpreter.take_repeat(
                (HOLD_REPEAT_DELAY + HOLD_REPEAT_INTERVAL)
                    .checked_sub(Duration::from_nanos(1))
                    .unwrap()
            ),
            None
        );
        assert_eq!(
            interpreter.take_repeat(HOLD_REPEAT_DELAY + HOLD_REPEAT_INTERVAL),
            Some(StockAction::StepDisplayBrightness(StepDirection::Down))
        );
        assert!(
            interpreter
                .ingest_at(
                    &input(2, false, Vec::new()),
                    None,
                    HOLD_REPEAT_DELAY + HOLD_REPEAT_INTERVAL
                )
                .unwrap()
                .actions
                .is_empty()
        );
    }

    #[test]
    fn request_ids_skip_zero_and_pending_values_after_wrap() {
        let mut ids = RequestIds { next: u32::MAX };
        let mut pending = BTreeMap::new();
        pending.insert(u32::MAX, PendingRequest::TapKeys);

        assert_eq!(ids.next(&pending), Ok(1));
        assert_eq!(ids.next(&pending), Ok(2));
    }

    #[test]
    fn every_stock_action_has_the_exact_wire_key() {
        let expected = [
            (StockAction::Escape, KeyCode::Escape),
            (StockAction::Function(FunctionKey::F1), KeyCode::F1),
            (StockAction::Function(FunctionKey::F2), KeyCode::F2),
            (StockAction::Function(FunctionKey::F3), KeyCode::F3),
            (StockAction::Function(FunctionKey::F4), KeyCode::F4),
            (StockAction::Function(FunctionKey::F5), KeyCode::F5),
            (StockAction::Function(FunctionKey::F6), KeyCode::F6),
            (StockAction::Function(FunctionKey::F7), KeyCode::F7),
            (StockAction::Function(FunctionKey::F8), KeyCode::F8),
            (StockAction::Function(FunctionKey::F9), KeyCode::F9),
            (StockAction::Function(FunctionKey::F10), KeyCode::F10),
            (StockAction::Function(FunctionKey::F11), KeyCode::F11),
            (StockAction::Function(FunctionKey::F12), KeyCode::F12),
        ];
        for (action, key) in expected {
            assert_eq!(action_key(action), Some(key));
        }
    }
}
