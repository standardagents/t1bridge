#[cfg(any(
    feature = "import-fs",
    feature = "seqpacket",
    feature = "touchbar-session"
))]
use std::ffi::c_char;
#[cfg(any(
    feature = "frame-memfd",
    feature = "import-fs",
    feature = "preserved-efi",
    feature = "sep-operation",
    feature = "seqpacket",
    feature = "touchbar-drm",
    feature = "touchbar-io",
    feature = "touchbar-session",
    feature = "usb-cycle-guard",
    feature = "xz"
))]
use std::ffi::c_int;
#[cfg(any(
    feature = "secret-wipe",
    feature = "sep-operation",
    feature = "seqpacket"
))]
use std::ffi::c_void;
#[cfg(feature = "sep-operation")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "sep-operation")]
type RawKeystoreObserver = unsafe extern "C" fn(u8, c_int, i8, i32);
#[cfg(feature = "sep-operation")]
type RawSepCleanupObserver = unsafe extern "C" fn(c_int);

#[cfg(any(feature = "frame-memfd", feature = "seqpacket"))]
use std::os::fd::AsRawFd;
#[cfg(any(
    feature = "frame-memfd",
    feature = "seqpacket",
    feature = "touchbar-session"
))]
use std::os::fd::BorrowedFd;
#[cfg(any(feature = "frame-memfd", feature = "touchbar-session"))]
use std::ptr::NonNull;

#[cfg(feature = "frame-memfd")]
#[repr(C)]
pub(super) struct RawFrameMemfdRenderer {
    _private: [u8; 0],
}

#[cfg(feature = "frame-memfd")]
#[repr(C)]
pub(super) struct RawFrameMemfdReader {
    _private: [u8; 0],
}

#[cfg(feature = "touchbar-drm")]
#[repr(C)]
pub(super) struct RawTouchBarDrm {
    _private: [u8; 0],
}

#[cfg(feature = "touchbar-drm")]
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct RawTouchBarDrmGeometry {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub byte_length: u64,
}

#[cfg(feature = "touchbar-drm")]
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct RawTouchBarDrmDamageRectangle {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[cfg(feature = "touchbar-io")]
#[repr(C)]
pub(super) struct RawTouchBarFnEdge {
    pub pressed: c_int,
}

#[cfg(feature = "touchbar-io")]
#[repr(C)]
pub(super) struct RawTouchBarKeyboard {
    _private: [u8; 0],
}

#[cfg(feature = "touchbar-session")]
#[repr(C)]
pub(super) struct RawTouchBarSessionWatch {
    _private: [u8; 0],
}

#[cfg(feature = "import-fs")]
#[repr(C)]
pub(super) struct RawImportFs {
    _private: [u8; 0],
}

#[cfg(feature = "import-fs")]
#[repr(C)]
pub(super) struct RawImportFsComponent {
    pub name: *const c_char,
    pub mode: u32,
}

#[cfg(any(feature = "seqpacket", feature = "touchbar-session"))]
use std::ffi::CStr;
#[cfg(any(
    feature = "preserved-efi",
    feature = "seqpacket",
    feature = "touchbar-io"
))]
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

#[cfg(feature = "seqpacket")]
#[repr(C)]
#[allow(clippy::struct_field_names)]
pub(super) struct RawCredentials {
    pub process_id: c_int,
    pub user_id: u32,
    pub group_id: u32,
}

#[cfg(feature = "sep-operation")]
type RawSepCancelled = unsafe extern "C" fn(context: *mut c_void) -> c_int;

#[cfg(feature = "sep-operation")]
type RawSepCredentialCallback = unsafe extern "C" fn(
    context: *mut c_void,
    credential: *const u8,
    credential_length: usize,
) -> c_int;

#[cfg(feature = "sep-operation")]
type RawSepKeybagCallback = unsafe extern "C" fn(
    context: *mut c_void,
    disposition: c_int,
    credential: *const u8,
    credential_length: usize,
) -> c_int;

#[cfg(feature = "sep-operation")]
type RawSepReadyCallback = unsafe extern "C" fn(context: *mut c_void) -> c_int;

#[cfg(feature = "sep-operation")]
type RawSepPreparedCleanup = unsafe extern "C" fn(context: *mut c_void);

unsafe extern "C" {
    #[cfg(feature = "frame-memfd")]
    fn t1_frame_memfd_renderer_create(
        output: *mut *mut RawFrameMemfdRenderer,
        byte_length: u64,
    ) -> c_int;
    #[cfg(feature = "frame-memfd")]
    fn t1_frame_memfd_renderer_descriptor(frame: *const RawFrameMemfdRenderer) -> c_int;
    #[cfg(feature = "frame-memfd")]
    fn t1_frame_memfd_renderer_mapping(frame: *mut RawFrameMemfdRenderer) -> *mut u8;
    #[cfg(feature = "frame-memfd")]
    fn t1_frame_memfd_renderer_length(frame: *const RawFrameMemfdRenderer) -> usize;
    #[cfg(feature = "frame-memfd")]
    fn t1_frame_memfd_renderer_destroy(frame: *mut RawFrameMemfdRenderer);
    #[cfg(feature = "frame-memfd")]
    fn t1_frame_memfd_reader_accept(
        output: *mut *mut RawFrameMemfdReader,
        received_descriptor: c_int,
        expected_byte_length: u64,
    ) -> c_int;
    #[cfg(feature = "frame-memfd")]
    fn t1_frame_memfd_reader_length(frame: *const RawFrameMemfdReader) -> usize;
    #[cfg(feature = "frame-memfd")]
    fn t1_frame_memfd_reader_copy(
        frame: *const RawFrameMemfdReader,
        destination: *mut u8,
        destination_length: usize,
    ) -> c_int;
    #[cfg(feature = "frame-memfd")]
    fn t1_frame_memfd_reader_destroy(frame: *mut RawFrameMemfdReader);

    #[cfg(feature = "secret-wipe")]
    fn t1_secret_wipe(buffer: *mut c_void, size: usize);

    #[cfg(feature = "sep-operation")]
    fn sep_operation_run_authorized(
        timeout_ms: u32,
        audit_uid: u32,
        cancelled: Option<RawSepCancelled>,
        cancellation_context: *mut c_void,
        callback: Option<RawSepCredentialCallback>,
        callback_context: *mut c_void,
    ) -> c_int;

    #[cfg(feature = "sep-operation")]
    fn sep_keybag_run(
        mode: c_int,
        authorization: c_int,
        timeout_ms: u32,
        cancelled: Option<RawSepCancelled>,
        cancellation_context: *mut c_void,
        callback: Option<RawSepKeybagCallback>,
        callback_context: *mut c_void,
    ) -> c_int;

    #[cfg(feature = "sep-operation")]
    fn sep_keybag_run_prepared(
        mode: c_int,
        authorization: c_int,
        timeout_ms: u32,
        cancelled: Option<RawSepCancelled>,
        cancellation_context: *mut c_void,
        prepare: Option<RawSepReadyCallback>,
        prepare_context: *mut c_void,
        callback: Option<RawSepKeybagCallback>,
        callback_context: *mut c_void,
        cleanup: Option<RawSepPreparedCleanup>,
        cleanup_context: *mut c_void,
    ) -> c_int;

    #[cfg(feature = "sep-operation")]
    fn sep_keybag_run_notification_relay(
        acquisition_timeout_ms: u32,
        poll_timeout_ms: u32,
        cancelled: Option<RawSepCancelled>,
        cancellation_context: *mut c_void,
        ready: Option<RawSepReadyCallback>,
        ready_context: *mut c_void,
    ) -> c_int;

    #[cfg(feature = "sep-operation")]
    fn sep_operation_set_cleanup_observer(
        observer: Option<RawSepCleanupObserver>,
    ) -> Option<RawSepCleanupObserver>;

    #[cfg(feature = "sep-operation")]
    fn sep_session_set_keystore_observer(
        observer: Option<RawKeystoreObserver>,
    ) -> Option<RawKeystoreObserver>;

    #[cfg(feature = "import-fs")]
    fn t1_import_fs_reserve(
        output: *mut *mut RawImportFs,
        anchor_descriptor: c_int,
        components: *const RawImportFsComponent,
        component_count: usize,
        record_size: usize,
        record_limit: usize,
    ) -> c_int;
    #[cfg(feature = "import-fs")]
    fn t1_import_fs_inspect_destination(
        filesystem: *mut RawImportFs,
        expected: *const u8,
        expected_size: usize,
        state: *mut u32,
    ) -> c_int;
    #[cfg(feature = "import-fs")]
    fn t1_import_fs_inspect_orphan(filesystem: *mut RawImportFs, state: *mut u32) -> c_int;
    #[cfg(feature = "import-fs")]
    fn t1_import_fs_remove_validated_orphan(filesystem: *mut RawImportFs) -> c_int;
    #[cfg(feature = "import-fs")]
    fn t1_import_fs_create_private_temporary(filesystem: *mut RawImportFs) -> c_int;
    #[cfg(feature = "import-fs")]
    fn t1_import_fs_write_temporary(
        filesystem: *mut RawImportFs,
        data: *const u8,
        size: usize,
    ) -> c_int;
    #[cfg(feature = "import-fs")]
    fn t1_import_fs_sync_temporary(filesystem: *mut RawImportFs) -> c_int;
    #[cfg(feature = "import-fs")]
    fn t1_import_fs_rename_temporary(filesystem: *mut RawImportFs) -> c_int;
    #[cfg(feature = "import-fs")]
    fn t1_import_fs_sync_destination_directory(filesystem: *mut RawImportFs) -> c_int;
    #[cfg(feature = "import-fs")]
    fn t1_import_fs_close(filesystem: *mut RawImportFs);

    #[cfg(feature = "preserved-efi")]
    fn t1_preserved_efi_open_fdr(
        root_descriptor: c_int,
        source_descriptor: *mut c_int,
        source_size: *mut u64,
    ) -> c_int;

    #[cfg(feature = "preserved-efi")]
    fn t1_preserved_efi_open_backup(
        path: *const std::ffi::c_char,
        source_descriptor: *mut c_int,
        source_size: *mut u64,
    ) -> c_int;

    #[cfg(feature = "preserved-efi-discovery")]
    fn t1_efi_roots_discover(
        root_descriptors: *mut c_int,
        descriptor_capacity: usize,
        root_count: *mut usize,
    ) -> c_int;
    #[cfg(feature = "preserved-efi-discovery")]
    fn t1_efi_roots_is_root() -> c_int;

    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_adopt_systemd_listener(
        activation_pid: c_int,
        activation_fds: u32,
        expected_path: *const c_char,
        listener_descriptor: *mut c_int,
    ) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_adopt_systemd_listener_pair(
        activation_pid: c_int,
        activation_fds: u32,
        first_expected_path: *const c_char,
        second_expected_path: *const c_char,
        first_listener_descriptor: *mut c_int,
        second_listener_descriptor: *mut c_int,
    ) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_listener_ready(listener_descriptor: c_int, ready: *mut c_int) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_accept(listener_descriptor: c_int, client_descriptor: *mut c_int) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_connect_touchbar(client_descriptor: *mut c_int) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_connect_auth(client_descriptor: *mut c_int) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_peer_credentials(
        client_descriptor: c_int,
        credentials: *mut RawCredentials,
    ) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_peer_in_group(client_descriptor: c_int, allowed_gid: u32) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_peer_closed(client_descriptor: c_int, peer_closed: *mut c_int) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_receive_with_fds(
        client_descriptor: c_int,
        buffer: *mut c_void,
        capacity: usize,
        received_length: *mut usize,
        descriptors: *mut c_int,
        descriptor_count: usize,
    ) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_receive_at_most_one_fd(
        client_descriptor: c_int,
        buffer: *mut c_void,
        capacity: usize,
        received_length: *mut usize,
        descriptor: *mut c_int,
        received_descriptor_count: *mut usize,
    ) -> c_int;
    #[cfg(feature = "seqpacket")]
    fn t1_seqpacket_send_with_fds(
        client_descriptor: c_int,
        packet: *const c_void,
        packet_length: usize,
        descriptors: *const c_int,
        descriptor_count: usize,
    ) -> c_int;

    #[cfg(feature = "seqpacket")]
    #[link_name = "getegid"]
    fn c_getegid() -> u32;

    #[cfg(all(feature = "seqpacket", any(test, feature = "seqpacket-test-support")))]
    #[link_name = "socketpair"]
    fn c_socketpair(domain: c_int, socket_type: c_int, protocol: c_int, pair: *mut c_int) -> c_int;
    #[cfg(all(feature = "seqpacket", test))]
    #[link_name = "fcntl"]
    fn c_fcntl(descriptor: c_int, command: c_int, ...) -> c_int;

    #[cfg(feature = "touchbar-drm")]
    fn t1_touchbar_drm_open(
        output: *mut *mut RawTouchBarDrm,
        geometry: *mut RawTouchBarDrmGeometry,
    ) -> c_int;
    #[cfg(feature = "touchbar-drm")]
    fn t1_touchbar_drm_present(
        display: *mut RawTouchBarDrm,
        pixels: *const u8,
        pixel_length: usize,
    ) -> c_int;
    #[cfg(feature = "touchbar-drm")]
    fn t1_touchbar_drm_present_rectangles(
        display: *mut RawTouchBarDrm,
        pixels: *const u8,
        pixel_length: usize,
        rectangles: *const RawTouchBarDrmDamageRectangle,
        rectangle_count: usize,
    ) -> c_int;
    #[cfg(feature = "touchbar-drm")]
    fn t1_touchbar_drm_close(display: *mut RawTouchBarDrm);

    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_digitizer_open(descriptor: *mut c_int) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_digitizer_read(descriptor: c_int, timeout_ms: u32, report: *mut u8) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_fn_open(descriptor: *mut c_int) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_fn_state(descriptor: c_int, pressed: *mut c_int) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_fn_read(
        descriptor: c_int,
        edges: *mut RawTouchBarFnEdge,
        capacity: usize,
        edge_count: *mut usize,
    ) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_wait_inputs(
        digitizer_descriptor: c_int,
        fn_descriptor: c_int,
        timeout_ms: u32,
        ready_inputs: *mut u32,
    ) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_wait_events(
        digitizer_descriptor: c_int,
        fn_descriptor: c_int,
        client_descriptor: c_int,
        client_interest: u32,
        timeout_ms: u32,
        ready_events: *mut u32,
    ) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_uinput_create(device: *mut *mut RawTouchBarKeyboard) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_uinput_tap(device: *mut RawTouchBarKeyboard, key: c_int) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_uinput_release_all(device: *mut RawTouchBarKeyboard) -> c_int;
    #[cfg(feature = "touchbar-io")]
    fn t1_touchbar_uinput_close(device: *mut RawTouchBarKeyboard) -> c_int;

    #[cfg(feature = "touchbar-session")]
    fn t1_touchbar_session_watch_create(output: *mut *mut RawTouchBarSessionWatch) -> c_int;
    #[cfg(feature = "touchbar-session")]
    fn t1_touchbar_session_watch_poll_source(
        watch: *mut RawTouchBarSessionWatch,
        descriptor: *mut c_int,
        events: *mut c_int,
        timeout_usec: *mut u64,
    ) -> c_int;
    #[cfg(feature = "touchbar-session")]
    fn t1_touchbar_session_watch_admit(watch: *mut RawTouchBarSessionWatch, peer_uid: u32)
    -> c_int;
    #[cfg(feature = "touchbar-session")]
    fn t1_touchbar_session_watch_refresh(watch: *mut RawTouchBarSessionWatch) -> c_int;
    #[cfg(feature = "touchbar-session")]
    fn t1_touchbar_session_watch_set_brightness(
        watch: *mut RawTouchBarSessionWatch,
        subsystem: *const c_char,
        name: *const c_char,
        value: u32,
    ) -> c_int;
    #[cfg(feature = "touchbar-session")]
    fn t1_touchbar_session_watch_is_admitted(watch: *const RawTouchBarSessionWatch) -> c_int;
    #[cfg(feature = "touchbar-session")]
    fn t1_touchbar_session_watch_release(watch: *mut RawTouchBarSessionWatch);
    #[cfg(feature = "touchbar-session")]
    fn t1_touchbar_session_watch_destroy(watch: *mut RawTouchBarSessionWatch);

    #[cfg(feature = "usb-cycle-guard")]
    fn t1_usb_cycle_guard_acquire() -> c_int;
    #[cfg(feature = "usb-cycle-guard")]
    fn t1_usb_cycle_guard_acquire_sep() -> c_int;
    #[cfg(feature = "usb-cycle-guard")]
    fn t1_usb_cycle_guard_interrupted() -> c_int;
    #[cfg(feature = "usb-cycle-guard")]
    fn t1_usb_cycle_guard_release_sep();
    #[cfg(feature = "usb-cycle-guard")]
    fn t1_usb_cycle_guard_release();

    #[cfg(feature = "xz")]
    fn t1_xz_decode(
        input: *const u8,
        input_size: usize,
        output: *mut u8,
        output_capacity: usize,
        memory_limit: u64,
        output_size: *mut usize,
    ) -> c_int;
}

#[cfg(feature = "usb-cycle-guard")]
pub(super) fn acquire_usb_cycle_guard() -> c_int {
    // SAFETY: the native boundary accepts no pointers, owns both fixed lock
    // descriptors on success, and installs only deferred signal handlers.
    unsafe { t1_usb_cycle_guard_acquire() }
}

#[cfg(feature = "usb-cycle-guard")]
pub(super) fn usb_cycle_guard_interrupted() -> bool {
    // SAFETY: this reads one native sig_atomic_t flag and retains no state.
    unsafe { t1_usb_cycle_guard_interrupted() == 1 }
}

#[cfg(feature = "usb-cycle-guard")]
pub(super) fn acquire_usb_cycle_sep_guard() -> c_int {
    // SAFETY: a live cycle Guard owns native global state and calls this at
    // most once after competing SEP services have stopped.
    unsafe { t1_usb_cycle_guard_acquire_sep() }
}

#[cfg(feature = "usb-cycle-guard")]
pub(super) fn release_usb_cycle_sep_guard() {
    // SAFETY: the live cycle Guard owns the SEP lock when this is called. The
    // native release is idempotent and retains the independent cycle lock.
    unsafe { t1_usb_cycle_guard_release_sep() };
}

#[cfg(feature = "usb-cycle-guard")]
pub(super) fn release_usb_cycle_guard() {
    // SAFETY: the safe Guard owner calls this exactly once on drop after a
    // successful acquisition. The native release is also idempotent.
    unsafe { t1_usb_cycle_guard_release() };
}

#[cfg(feature = "frame-memfd")]
pub(super) fn create_frame_memfd_renderer(byte_length: u64) -> (c_int, *mut RawFrameMemfdRenderer) {
    let mut frame = std::ptr::null_mut();
    // SAFETY: `frame` is writable for one pointer. Success transfers one
    // exclusively owned handle whose mapping and descriptor are released by
    // `destroy_frame_memfd_renderer`; no output is retained on failure.
    let status = unsafe { t1_frame_memfd_renderer_create(&raw mut frame, byte_length) };
    (status, frame)
}

#[cfg(feature = "frame-memfd")]
pub(super) fn frame_memfd_renderer_descriptor(
    frame: &NonNull<RawFrameMemfdRenderer>,
) -> BorrowedFd<'_> {
    // SAFETY: `frame` is the live, exclusively owned native handle. The C
    // boundary returns its borrowed descriptor without transferring ownership.
    let descriptor = unsafe { t1_frame_memfd_renderer_descriptor(frame.as_ptr()) };
    debug_assert!(descriptor >= 0);
    // SAFETY: the C contract guarantees that a live, non-null renderer handle
    // returns its open descriptor. The returned lifetime is tied to the handle.
    unsafe { BorrowedFd::borrow_raw(descriptor) }
}

#[cfg(feature = "frame-memfd")]
pub(super) fn frame_memfd_renderer_bytes(frame: &mut NonNull<RawFrameMemfdRenderer>) -> &mut [u8] {
    // SAFETY: the exclusive handle borrow prevents another Rust slice from
    // aliasing this writable mapping. A successfully created native handle
    // always has one non-null mapping of exactly its reported positive length.
    unsafe {
        let length = t1_frame_memfd_renderer_length(frame.as_ptr());
        let mapping = t1_frame_memfd_renderer_mapping(frame.as_ptr());
        std::slice::from_raw_parts_mut(mapping, length)
    }
}

#[cfg(feature = "frame-memfd")]
pub(super) fn frame_memfd_renderer_length(frame: &NonNull<RawFrameMemfdRenderer>) -> usize {
    // SAFETY: `frame` is a live native handle and this query retains nothing.
    unsafe { t1_frame_memfd_renderer_length(frame.as_ptr()) }
}

#[cfg(feature = "frame-memfd")]
pub(super) fn destroy_frame_memfd_renderer(frame: NonNull<RawFrameMemfdRenderer>) {
    // SAFETY: the safe owner transfers its unique handle exactly once on drop.
    unsafe { t1_frame_memfd_renderer_destroy(frame.as_ptr()) };
}

#[cfg(feature = "frame-memfd")]
pub(super) fn accept_frame_memfd_reader(
    received_descriptor: BorrowedFd<'_>,
    expected_byte_length: u64,
) -> (c_int, *mut RawFrameMemfdReader) {
    let mut frame = std::ptr::null_mut();
    // SAFETY: `frame` is writable for one pointer and the received descriptor
    // remains borrowed and open for the call. Success returns a distinct owned
    // duplicate and read-only mapping; failure retains neither.
    let status = unsafe {
        t1_frame_memfd_reader_accept(
            &raw mut frame,
            received_descriptor.as_raw_fd(),
            expected_byte_length,
        )
    };
    (status, frame)
}

#[cfg(feature = "frame-memfd")]
pub(super) fn copy_frame_memfd_reader(
    frame: &NonNull<RawFrameMemfdReader>,
    destination: &mut [u8],
) -> c_int {
    // SAFETY: `frame` is a live native handle and `destination` is exclusively
    // writable for its reported length. Native volatile reads copy shared
    // bytes without constructing a Rust reference to externally mutable data.
    unsafe {
        t1_frame_memfd_reader_copy(frame.as_ptr(), destination.as_mut_ptr(), destination.len())
    }
}

#[cfg(feature = "frame-memfd")]
pub(super) fn frame_memfd_reader_length(frame: &NonNull<RawFrameMemfdReader>) -> usize {
    // SAFETY: `frame` is a live native handle and this query retains nothing.
    unsafe { t1_frame_memfd_reader_length(frame.as_ptr()) }
}

#[cfg(feature = "frame-memfd")]
pub(super) fn destroy_frame_memfd_reader(frame: NonNull<RawFrameMemfdReader>) {
    // SAFETY: the safe owner transfers its unique handle exactly once on drop.
    unsafe { t1_frame_memfd_reader_destroy(frame.as_ptr()) };
}

#[cfg(feature = "secret-wipe")]
pub(super) fn wipe_secret(bytes: &mut [u8]) {
    // SAFETY: the slice is writable for its reported length and remains alive
    // for the complete call. The native function retains no pointer.
    unsafe { t1_secret_wipe(bytes.as_mut_ptr().cast(), bytes.len()) };
}

#[cfg(feature = "sep-operation")]
struct SepCallbackState<F, T> {
    callback: Option<F>,
    output: Option<T>,
    panic: Option<Box<dyn std::any::Any + Send>>,
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn sep_cancelled(context: *mut c_void) -> c_int {
    if context.is_null() {
        return 1;
    }
    // SAFETY: `run_sep_authorized` passes a live `AtomicBool` for the complete
    // synchronous native call. The native boundary retains no pointer.
    i32::from(unsafe { &*context.cast::<AtomicBool>() }.load(Ordering::Acquire))
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn sep_credential_callback<F, T>(
    context: *mut c_void,
    credential: *const u8,
    credential_length: usize,
) -> c_int
where
    F: FnOnce(&[u8; 16]) -> T,
{
    if context.is_null() || credential.is_null() || credential_length != 16 {
        return 1;
    }
    // SAFETY: the context is the live callback state owned by
    // `run_sep_authorized`. Native code lends exactly 16 initialized bytes for
    // this callback and wipes them after it returns.
    let state = unsafe { &mut *context.cast::<SepCallbackState<F, T>>() };
    let credential = unsafe { &*credential.cast::<[u8; 16]>() };
    let Some(callback) = state.callback.take() else {
        return 1;
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(credential))) {
        Ok(output) => {
            state.output = Some(output);
            0
        }
        Err(panic) => {
            state.panic = Some(panic);
            1
        }
    }
}

#[cfg(feature = "sep-operation")]
pub(super) fn run_sep_authorized<F, T>(
    timeout_ms: u32,
    audit_uid: u32,
    cancellation: &AtomicBool,
    callback: F,
) -> (c_int, Option<T>)
where
    F: FnOnce(&[u8; 16]) -> T,
{
    let _cleanup_observer = SepCleanupObservation::install(crate::diagnostics::enabled());
    let mut state = SepCallbackState {
        callback: Some(callback),
        output: None,
        panic: None,
    };
    // SAFETY: both context pointers refer to live, correctly typed values for
    // the complete synchronous call. Both trampolines validate native pointer
    // and length values, retain nothing, and contain callback unwinding.
    let status = unsafe {
        sep_operation_run_authorized(
            timeout_ms,
            audit_uid,
            Some(sep_cancelled),
            std::ptr::from_ref(cancellation).cast_mut().cast(),
            Some(sep_credential_callback::<F, T>),
            std::ptr::from_mut(&mut state).cast(),
        )
    };
    if let Some(panic) = state.panic {
        std::panic::resume_unwind(panic);
    }
    (status, state.output)
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn sep_keybag_callback<F, T>(
    context: *mut c_void,
    disposition: c_int,
    credential: *const u8,
    credential_length: usize,
) -> c_int
where
    F: FnOnce(c_int, &[u8; 16]) -> T,
{
    if context.is_null()
        || credential.is_null()
        || credential_length != 16
        || !matches!(disposition, 0 | 1)
    {
        return 1;
    }
    // SAFETY: `run_sep_keybag` owns this state for the synchronous native
    // call. Native code lends exactly 16 initialized bytes and a validated
    // disposition only during this callback.
    let state = unsafe { &mut *context.cast::<SepCallbackState<F, (c_int, T)>>() };
    let credential = unsafe { &*credential.cast::<[u8; 16]>() };
    let Some(callback) = state.callback.take() else {
        return 1;
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        callback(disposition, credential)
    })) {
        Ok(output) => {
            state.output = Some((disposition, output));
            0
        }
        Err(panic) => {
            state.panic = Some(panic);
            1
        }
    }
}

#[cfg(feature = "sep-operation")]
pub(super) fn run_sep_keybag<F, T>(
    mode: c_int,
    authorization: c_int,
    timeout_ms: u32,
    cancellation: &AtomicBool,
    callback: F,
) -> (c_int, Option<(c_int, T)>)
where
    F: FnOnce(c_int, &[u8; 16]) -> T,
{
    let _cleanup_observer = SepCleanupObservation::install(crate::diagnostics::enabled());
    let mut state = SepCallbackState {
        callback: Some(callback),
        output: None,
        panic: None,
    };
    // SAFETY: both contexts remain live for this synchronous call. Native code
    // retains neither; callback panics are contained until native teardown.
    let status = unsafe {
        sep_keybag_run(
            mode,
            authorization,
            timeout_ms,
            Some(sep_cancelled),
            std::ptr::from_ref(cancellation).cast_mut().cast(),
            Some(sep_keybag_callback::<F, T>),
            std::ptr::from_mut(&mut state).cast(),
        )
    };
    if let Some(panic) = state.panic {
        std::panic::resume_unwind(panic);
    }
    (status, state.output)
}

#[cfg(feature = "sep-operation")]
struct SepPreparedKeybagState<Prepare, Operation, Prepared, PreparationError, Output> {
    prepare: Option<Prepare>,
    operation: Option<Operation>,
    prepared: Option<Prepared>,
    preparation_error: Option<PreparationError>,
    output: Option<(c_int, Output)>,
    panic: Option<Box<dyn std::any::Any + Send>>,
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn sep_prepare_keybag<Prepare, Operation, Prepared, PreparationError, Output>(
    context: *mut c_void,
) -> c_int
where
    Prepare: FnOnce() -> Result<Prepared, PreparationError>,
{
    if context.is_null() {
        return 1;
    }
    let state = unsafe {
        &mut *context
            .cast::<SepPreparedKeybagState<Prepare, Operation, Prepared, PreparationError, Output>>(
            )
    };
    let Some(prepare) = state.prepare.take() else {
        return 1;
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(prepare)) {
        Ok(Ok(prepared)) => {
            state.prepared = Some(prepared);
            0
        }
        Ok(Err(error)) => {
            state.preparation_error = Some(error);
            1
        }
        Err(panic) => {
            state.panic = Some(panic);
            1
        }
    }
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn sep_prepared_keybag_callback<
    Prepare,
    Operation,
    Prepared,
    PreparationError,
    Output,
>(
    context: *mut c_void,
    disposition: c_int,
    credential: *const u8,
    credential_length: usize,
) -> c_int
where
    Operation: FnOnce(&mut Prepared, c_int, &[u8; 16]) -> Output,
{
    if context.is_null()
        || credential.is_null()
        || credential_length != 16
        || !matches!(disposition, 0 | 1)
    {
        return 1;
    }
    let state = unsafe {
        &mut *context
            .cast::<SepPreparedKeybagState<Prepare, Operation, Prepared, PreparationError, Output>>(
            )
    };
    let credential = unsafe { &*credential.cast::<[u8; 16]>() };
    let (Some(operation), Some(prepared)) = (state.operation.take(), state.prepared.as_mut())
    else {
        return 1;
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        operation(prepared, disposition, credential)
    })) {
        Ok(output) => {
            state.output = Some((disposition, output));
            0
        }
        Err(panic) => {
            state.panic = Some(panic);
            1
        }
    }
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn sep_cleanup_prepared_keybag<
    Prepare,
    Operation,
    Prepared,
    PreparationError,
    Output,
>(
    context: *mut c_void,
) {
    if context.is_null() {
        return;
    }
    let state = unsafe {
        &mut *context
            .cast::<SepPreparedKeybagState<Prepare, Operation, Prepared, PreparationError, Output>>(
            )
    };
    let Some(prepared) = state.prepared.take() else {
        return;
    };
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(prepared)))
        && state.panic.is_none()
    {
        state.panic = Some(panic);
    }
}

#[cfg(feature = "sep-operation")]
pub(super) fn run_sep_prepared_keybag<Prepare, Operation, Prepared, PreparationError, Output>(
    mode: c_int,
    authorization: c_int,
    timeout_ms: u32,
    cancellation: &AtomicBool,
    prepare: Prepare,
    operation: Operation,
) -> (
    c_int,
    Option<PreparationError>,
    Option<(c_int, Output)>,
    bool,
)
where
    Prepare: FnOnce() -> Result<Prepared, PreparationError>,
    Operation: FnOnce(&mut Prepared, c_int, &[u8; 16]) -> Output,
{
    let _cleanup_observer = SepCleanupObservation::install(crate::diagnostics::enabled());
    let mut state: SepPreparedKeybagState<Prepare, Operation, Prepared, PreparationError, Output> =
        SepPreparedKeybagState {
            prepare: Some(prepare),
            operation: Some(operation),
            prepared: None,
            preparation_error: None,
            output: None,
            panic: None,
        };
    let context = std::ptr::from_mut(&mut state).cast();
    let status = unsafe {
        sep_keybag_run_prepared(
            mode,
            authorization,
            timeout_ms,
            Some(sep_cancelled),
            std::ptr::from_ref(cancellation).cast_mut().cast(),
            Some(sep_prepare_keybag::<Prepare, Operation, Prepared, PreparationError, Output>),
            context,
            Some(
                sep_prepared_keybag_callback::<
                    Prepare,
                    Operation,
                    Prepared,
                    PreparationError,
                    Output,
                >,
            ),
            context,
            Some(
                sep_cleanup_prepared_keybag::<Prepare, Operation, Prepared, PreparationError, Output>,
            ),
            context,
        )
    };
    if let Some(panic) = state.panic {
        std::panic::resume_unwind(panic);
    }
    let prepared_released = state.prepared.is_none();
    (
        status,
        state.preparation_error,
        state.output,
        prepared_released,
    )
}

#[cfg(feature = "sep-operation")]
struct SepRelayCallbackState<'cancellation, C, F> {
    cancellation: &'cancellation C,
    ready: Option<F>,
    ready_called: bool,
    panic: Option<Box<dyn std::any::Any + Send>>,
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn sep_relay_cancelled<C, F>(context: *mut c_void) -> c_int
where
    C: Fn() -> bool,
{
    if context.is_null() {
        return 1;
    }
    // SAFETY: `run_sep_notification_relay` owns this state for the complete
    // synchronous call and the native boundary retains no pointer.
    let state = unsafe { &mut *context.cast::<SepRelayCallbackState<'_, C, F>>() };
    if state.panic.is_some() {
        return 1;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (state.cancellation)())) {
        Ok(cancelled) => i32::from(cancelled),
        Err(panic) => {
            state.panic = Some(panic);
            1
        }
    }
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn sep_ready_callback<C, F>(context: *mut c_void) -> c_int
where
    F: FnOnce() -> bool,
{
    if context.is_null() {
        return 1;
    }
    // SAFETY: `run_sep_notification_relay` owns this state for the entire
    // synchronous call. Native code invokes readiness at most once and retains
    // no pointer after returning.
    let state = unsafe { &mut *context.cast::<SepRelayCallbackState<'_, C, F>>() };
    let Some(callback) = state.ready.take() else {
        return 1;
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)) {
        Ok(true) => {
            state.ready_called = true;
            0
        }
        Ok(false) => 1,
        Err(panic) => {
            state.panic = Some(panic);
            1
        }
    }
}

#[cfg(feature = "sep-operation")]
struct SepCleanupObservation(Option<RawSepCleanupObserver>);

#[cfg(feature = "sep-operation")]
impl SepCleanupObservation {
    fn install(enabled: bool) -> Self {
        // SAFETY: a synchronous, thread-local, scalar-only hook; Drop restores
        // the previous hook on return or deferred Rust callback unwinding.
        Self(unsafe { sep_operation_set_cleanup_observer(enabled.then_some(observe_sep_cleanup)) })
    }
}

#[cfg(feature = "sep-operation")]
impl Drop for SepCleanupObservation {
    fn drop(&mut self) {
        // SAFETY: restore only the hook saved on this native caller's thread.
        unsafe { sep_operation_set_cleanup_observer(self.0) };
    }
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn observe_sep_cleanup(status: c_int) {
    // Never unwind across C or let diagnostics change an operation's result.
    let _ = std::panic::catch_unwind(|| {
        crate::diagnostics::sep_cleanup(status);
    });
}

#[cfg(feature = "sep-operation")]
unsafe extern "C" fn observe_keystore_reply(selector: u8, status: c_int, outer: i8, inner: i32) {
    // Diagnostics cannot unwind across C or change the operation's result.
    let _ = std::panic::catch_unwind(|| {
        crate::diagnostics::keystore_reply(selector, status, outer, inner);
    });
}

#[cfg(feature = "sep-operation")]
pub(super) fn run_sep_notification_relay<C, F>(
    acquisition_timeout_ms: u32,
    poll_timeout_ms: u32,
    cancellation: &C,
    ready: F,
) -> (c_int, bool)
where
    C: Fn() -> bool,
    F: FnOnce() -> bool,
{
    let _cleanup_observer = SepCleanupObservation::install(crate::diagnostics::enabled());
    let mut state = SepRelayCallbackState {
        cancellation,
        ready: Some(ready),
        ready_called: false,
        panic: None,
    };
    // SAFETY: the observer is thread-local, synchronous, and takes only scalars.
    // Save/restore brackets this native call, including a deferred callback panic.
    let previous = unsafe {
        sep_session_set_keystore_observer(
            crate::diagnostics::enabled().then_some(observe_keystore_reply),
        )
    };
    // SAFETY: both context pointers remain live for the synchronous native
    // lease. The callbacks retain nothing and Rust unwinding is contained
    // until native teardown has completed.
    let status = unsafe {
        sep_keybag_run_notification_relay(
            acquisition_timeout_ms,
            poll_timeout_ms,
            Some(sep_relay_cancelled::<C, F>),
            std::ptr::from_mut(&mut state).cast(),
            Some(sep_ready_callback::<C, F>),
            std::ptr::from_mut(&mut state).cast(),
        )
    };
    // SAFETY: restore this thread's prior observer before any Rust unwinding.
    unsafe { sep_session_set_keystore_observer(previous) };
    if let Some(panic) = state.panic {
        std::panic::resume_unwind(panic);
    }
    (status, state.ready_called)
}

#[cfg(feature = "import-fs")]
pub(super) fn reserve_import_fs(
    anchor: c_int,
    components: &[RawImportFsComponent],
    record_size: usize,
    record_limit: usize,
) -> (c_int, *mut RawImportFs) {
    let mut filesystem = std::ptr::null_mut();
    // SAFETY: `filesystem` is writable for one pointer, `components` remains
    // readable for the call, and each name pointer is owned by the safe wrapper.
    let status = unsafe {
        t1_import_fs_reserve(
            &raw mut filesystem,
            anchor,
            components.as_ptr(),
            components.len(),
            record_size,
            record_limit,
        )
    };
    (status, filesystem)
}

#[cfg(feature = "import-fs")]
pub(super) fn inspect_import_destination(
    filesystem: *mut RawImportFs,
    expected: &[u8],
) -> (c_int, u32) {
    let mut state = u32::MAX;
    // SAFETY: the native handle is exclusively owned by the safe wrapper;
    // `expected` and `state` remain readable/writable for the complete call.
    let status = unsafe {
        t1_import_fs_inspect_destination(
            filesystem,
            expected.as_ptr(),
            expected.len(),
            &raw mut state,
        )
    };
    (status, state)
}

#[cfg(feature = "import-fs")]
pub(super) fn inspect_import_orphan(filesystem: *mut RawImportFs) -> (c_int, u32) {
    let mut state = u32::MAX;
    // SAFETY: the native handle is exclusively owned by the safe wrapper and
    // `state` remains writable for the complete call.
    let status = unsafe { t1_import_fs_inspect_orphan(filesystem, &raw mut state) };
    (status, state)
}

#[cfg(feature = "import-fs")]
pub(super) fn remove_import_orphan(filesystem: *mut RawImportFs) -> c_int {
    // SAFETY: the native handle is exclusively owned by the safe wrapper.
    unsafe { t1_import_fs_remove_validated_orphan(filesystem) }
}

#[cfg(feature = "import-fs")]
pub(super) fn create_import_temporary(filesystem: *mut RawImportFs) -> c_int {
    // SAFETY: the native handle is exclusively owned by the safe wrapper.
    unsafe { t1_import_fs_create_private_temporary(filesystem) }
}

#[cfg(feature = "import-fs")]
pub(super) fn write_import_temporary(filesystem: *mut RawImportFs, data: &[u8]) -> c_int {
    // SAFETY: the native handle is exclusively owned by the safe wrapper and
    // `data` remains readable for the complete call.
    unsafe { t1_import_fs_write_temporary(filesystem, data.as_ptr(), data.len()) }
}

#[cfg(feature = "import-fs")]
pub(super) fn sync_import_temporary(filesystem: *mut RawImportFs) -> c_int {
    // SAFETY: the native handle is exclusively owned by the safe wrapper.
    unsafe { t1_import_fs_sync_temporary(filesystem) }
}

#[cfg(feature = "import-fs")]
pub(super) fn rename_import_temporary(filesystem: *mut RawImportFs) -> c_int {
    // SAFETY: the native handle is exclusively owned by the safe wrapper.
    unsafe { t1_import_fs_rename_temporary(filesystem) }
}

#[cfg(feature = "import-fs")]
pub(super) fn sync_import_directory(filesystem: *mut RawImportFs) -> c_int {
    // SAFETY: the native handle is exclusively owned by the safe wrapper.
    unsafe { t1_import_fs_sync_destination_directory(filesystem) }
}

#[cfg(feature = "import-fs")]
pub(super) fn close_import_fs(filesystem: *mut RawImportFs) {
    // SAFETY: the safe wrapper calls this exactly once for its owned handle.
    unsafe { t1_import_fs_close(filesystem) };
}

#[cfg(feature = "preserved-efi")]
pub(super) fn open_preserved_fdr(root: RawFd) -> (c_int, Option<(OwnedFd, u64)>) {
    let mut source = -1;
    let mut size = 0;
    // SAFETY: both output pointers remain writable for the complete call and
    // `root` remains borrowed and open. The native boundary returns a newly
    // owned descriptor only on success and retains no descriptor or pointer.
    let status = unsafe { t1_preserved_efi_open_fdr(root, &raw mut source, &raw mut size) };
    let opened = if status == 0 && source >= 0 {
        // SAFETY: the C contract returns a new owned descriptor only on
        // success; the explicit nonnegative check excludes sentinel values.
        Some((unsafe { OwnedFd::from_raw_fd(source) }, size))
    } else {
        None
    };
    (status, opened)
}

#[cfg(feature = "preserved-efi")]
pub(super) fn open_preserved_backup(path: &std::ffi::CStr) -> (c_int, Option<(OwnedFd, u64)>) {
    let mut source = -1;
    let mut size = 0;
    // SAFETY: path is NUL-terminated and live throughout the call; writable
    // outputs receive a newly owned descriptor only on success.
    let status =
        unsafe { t1_preserved_efi_open_backup(path.as_ptr(), &raw mut source, &raw mut size) };
    let opened = if status == 0 && source >= 0 {
        // SAFETY: success transfers this new descriptor exclusively to us.
        Some((unsafe { OwnedFd::from_raw_fd(source) }, size))
    } else {
        None
    };
    (status, opened)
}

#[cfg(feature = "preserved-efi-discovery")]
pub(super) fn discover_preserved_efi_roots(capacity: usize) -> (c_int, Option<Vec<OwnedFd>>) {
    let mut descriptors = [-1; 64];
    let mut count = 0_usize;
    if capacity > descriptors.len() {
        return (1, None);
    }
    // SAFETY: the descriptor array and count remain writable for the complete
    // call. The native contract either returns `count` newly owned descriptors
    // on success or closes every descriptor and returns none on failure.
    let status =
        unsafe { t1_efi_roots_discover(descriptors.as_mut_ptr(), capacity, &raw mut count) };
    if status != 0 || count > capacity {
        return (status, None);
    }
    let populated = &descriptors[..count];
    let invalid = populated
        .iter()
        .enumerate()
        .any(|(index, descriptor)| *descriptor < 0 || populated[..index].contains(descriptor));
    if invalid {
        for (index, descriptor) in populated.iter().copied().enumerate() {
            if descriptor >= 0 && !populated[..index].contains(&descriptor) {
                // SAFETY: every distinct nonnegative descriptor before `count`
                // is newly owned on native success. Immediate drop closes it.
                drop(unsafe { OwnedFd::from_raw_fd(descriptor) });
            }
        }
        return (5, None);
    }
    let mut roots = Vec::new();
    if roots.try_reserve_exact(count).is_err() {
        for descriptor in populated.iter().copied() {
            // SAFETY: validation above proves each descriptor is nonnegative,
            // distinct, and newly owned on native success.
            drop(unsafe { OwnedFd::from_raw_fd(descriptor) });
        }
        return (5, None);
    }
    for descriptor in populated.iter().copied() {
        // SAFETY: native success transfers one distinct owned descriptor in
        // each populated slot, and each slot is adopted exactly once here.
        roots.push(unsafe { OwnedFd::from_raw_fd(descriptor) });
    }
    (status, Some(roots))
}

#[cfg(feature = "preserved-efi-discovery")]
pub(super) fn preserved_efi_is_root() -> bool {
    // SAFETY: this argument-free native query reads only the effective UID and
    // returns zero or one without retaining process state.
    unsafe { t1_efi_roots_is_root() == 1 }
}

#[cfg(feature = "seqpacket")]
pub(super) fn adopt_systemd_listener(
    activation_pid: c_int,
    activation_fds: u32,
    expected_path: &CStr,
) -> (c_int, Option<OwnedFd>) {
    let mut listener = -1;
    // SAFETY: the numeric activation values are plain C integers;
    // `expected_path` is NUL-terminated and remains readable for the call;
    // `listener` is writable for one C `int`. Success transfers one new
    // descriptor after the native boundary closes inherited descriptor 3. The
    // native function never reads or mutates process environment state.
    let status = unsafe {
        t1_seqpacket_adopt_systemd_listener(
            activation_pid,
            activation_fds,
            expected_path.as_ptr(),
            &raw mut listener,
        )
    };
    let owned = if status == 0 {
        // SAFETY: the C contract returns a new owned descriptor only on success.
        Some(unsafe { OwnedFd::from_raw_fd(listener) })
    } else {
        None
    };
    (status, owned)
}

#[cfg(feature = "seqpacket")]
pub(super) fn adopt_systemd_listener_pair(
    activation_pid: c_int,
    activation_fds: u32,
    first_expected_path: &CStr,
    second_expected_path: &CStr,
) -> (c_int, Option<(OwnedFd, OwnedFd)>) {
    let mut first_listener = -1;
    let mut second_listener = -1;
    // SAFETY: both paths are NUL-terminated and remain readable for the call;
    // both descriptor outputs are writable C integers. Native success transfers
    // two distinct descriptors after closing inherited descriptors 3 and 4.
    let status = unsafe {
        t1_seqpacket_adopt_systemd_listener_pair(
            activation_pid,
            activation_fds,
            first_expected_path.as_ptr(),
            second_expected_path.as_ptr(),
            &raw mut first_listener,
            &raw mut second_listener,
        )
    };
    let owned = if status == 0 {
        // SAFETY: the native success contract returns two distinct nonnegative
        // owned descriptors, each adopted exactly once here.
        Some(unsafe {
            (
                OwnedFd::from_raw_fd(first_listener),
                OwnedFd::from_raw_fd(second_listener),
            )
        })
    } else {
        None
    };
    (status, owned)
}

#[cfg(feature = "seqpacket")]
pub(super) fn listener_ready(listener: RawFd) -> (c_int, bool) {
    let mut ready = 0;
    // SAFETY: `ready` is writable for one C `int`; the caller keeps the
    // borrowed listener open for the complete nonblocking probe.
    let status = unsafe { t1_seqpacket_listener_ready(listener, &raw mut ready) };
    (status, ready != 0)
}

#[cfg(feature = "seqpacket")]
pub(super) fn accept(listener: RawFd) -> (c_int, Option<OwnedFd>) {
    let mut client = -1;
    // SAFETY: `client` is writable for one C `int`; the borrowed listener
    // remains open for the call. Success transfers one newly accepted fd.
    let status = unsafe { t1_seqpacket_accept(listener, &raw mut client) };
    let owned = if status == 0 {
        // SAFETY: the C contract returns a new owned descriptor only on success.
        Some(unsafe { OwnedFd::from_raw_fd(client) })
    } else {
        None
    };
    (status, owned)
}

#[cfg(feature = "seqpacket")]
pub(super) fn connect_touchbar() -> (c_int, Option<OwnedFd>) {
    let mut client = -1;
    // SAFETY: `client` is writable for one C `int`. Exact native success
    // transfers one connected close-on-exec, nonblocking descriptor; every
    // failure closes any partial descriptor and leaves the sentinel intact.
    let status = unsafe { t1_seqpacket_connect_touchbar(&raw mut client) };
    let owned = if client >= 0 {
        // SAFETY: exact native success transfers one newly owned descriptor.
        Some(unsafe { OwnedFd::from_raw_fd(client) })
    } else {
        None
    };
    if status == 0 {
        (status, owned)
    } else {
        drop(owned);
        (status, None)
    }
}

#[cfg(feature = "seqpacket")]
pub(super) fn connect_auth() -> (c_int, Option<OwnedFd>) {
    let mut client = -1;
    // SAFETY: `client` is writable for one C `int`. Exact native success
    // transfers one connected close-on-exec, nonblocking descriptor; every
    // failure closes partial state and leaves the sentinel intact.
    let status = unsafe { t1_seqpacket_connect_auth(&raw mut client) };
    let owned = if client >= 0 {
        // SAFETY: exact native success transfers one newly owned descriptor.
        Some(unsafe { OwnedFd::from_raw_fd(client) })
    } else {
        None
    };
    if status == 0 {
        (status, owned)
    } else {
        drop(owned);
        (status, None)
    }
}

#[cfg(feature = "seqpacket")]
pub(super) fn peer_credentials(client: RawFd) -> (c_int, RawCredentials) {
    let mut credentials = RawCredentials {
        process_id: -1,
        user_id: u32::MAX,
        group_id: u32::MAX,
    };
    // SAFETY: `credentials` has the C layout and remains writable for the call;
    // `client` remains borrowed and open.
    let status = unsafe { t1_seqpacket_peer_credentials(client, &raw mut credentials) };
    if status == 0
        && (credentials.process_id <= 0
            || credentials.user_id == u32::MAX
            || credentials.group_id == u32::MAX)
    {
        return (9, credentials);
    }
    (status, credentials)
}

#[cfg(feature = "seqpacket")]
pub(super) fn peer_in_effective_group(client: RawFd) -> c_int {
    // SAFETY: `getegid` has no arguments or failure return and exposes only
    // this process's effective numeric group. The client descriptor remains
    // borrowed for the native kernel-credential check.
    let effective_group = unsafe { c_getegid() };
    if effective_group == 0 {
        return 19;
    }
    // SAFETY: the borrowed descriptor stays open for the call and the scalar
    // group identifier is passed by value. The native boundary retains none.
    unsafe { t1_seqpacket_peer_in_group(client, effective_group) }
}

#[cfg(all(test, feature = "seqpacket"))]
pub(super) fn effective_group_is_root_for_test() -> bool {
    // SAFETY: `getegid` has no arguments or failure return.
    unsafe { c_getegid() == 0 }
}

#[cfg(feature = "seqpacket")]
pub(super) fn peer_closed(client: RawFd) -> (c_int, bool) {
    let mut peer_closed = 0;
    // SAFETY: `peer_closed` is writable for one C `int`; `client` remains
    // borrowed and open. The C boundary writes only zero or one on success.
    let status = unsafe { t1_seqpacket_peer_closed(client, &raw mut peer_closed) };
    (status, peer_closed == 1)
}

#[cfg(feature = "seqpacket")]
pub(super) fn receive(client: RawFd, buffer: &mut [u8]) -> (c_int, usize) {
    let (status, received, descriptor) = receive_with_fd(client, buffer, false);
    debug_assert!(descriptor.is_none());
    (status, received)
}

#[cfg(feature = "seqpacket")]
pub(super) fn send(client: RawFd, packet: &[u8]) -> c_int {
    send_with_fd(client, packet, None)
}

#[cfg(feature = "seqpacket")]
pub(super) fn receive_with_fd(
    client: RawFd,
    buffer: &mut [u8],
    expect_descriptor: bool,
) -> (c_int, usize, Option<OwnedFd>) {
    let mut received = 0;
    let mut descriptor = -1;
    let (descriptors, descriptor_count) = if expect_descriptor {
        (&raw mut descriptor, 1)
    } else {
        (std::ptr::null_mut(), 0)
    };
    // SAFETY: the slice and scalar outputs remain writable for the complete
    // call and `client` remains borrowed and open. The native boundary accepts
    // exactly the declared zero-or-one descriptor count, closes every received
    // descriptor on error, and transfers one close-on-exec descriptor only on
    // exact one-descriptor success.
    let status = unsafe {
        t1_seqpacket_receive_with_fds(
            client,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut received,
            descriptors,
            descriptor_count,
        )
    };
    let owned = if status == 0 && expect_descriptor && descriptor >= 0 {
        // SAFETY: exact native success transfers this newly received
        // close-on-exec descriptor to the caller.
        Some(unsafe { OwnedFd::from_raw_fd(descriptor) })
    } else {
        None
    };
    (status, received, owned)
}

#[cfg(feature = "seqpacket")]
pub(super) fn receive_at_most_one_fd(
    client: RawFd,
    buffer: &mut [u8],
) -> (c_int, usize, Option<OwnedFd>) {
    let mut received = 0;
    let mut descriptor = -1;
    let mut descriptor_count = 0;
    // SAFETY: the slice and scalar outputs remain writable for the call and
    // `client` stays borrowed. Native success transfers zero or one
    // close-on-exec descriptor; every rejection closes all received fds.
    let status = unsafe {
        t1_seqpacket_receive_at_most_one_fd(
            client,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut received,
            &raw mut descriptor,
            &raw mut descriptor_count,
        )
    };
    let result = match (status, descriptor_count, descriptor) {
        (0, 0, -1) => (0, received, None),
        (0, 1, descriptor) if descriptor >= 0 => {
            // SAFETY: native success with count one transfers this descriptor.
            let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
            (0, received, Some(owned))
        }
        (_, _, descriptor) if descriptor >= 0 => {
            // SAFETY: a malformed native result still transferred this open
            // descriptor. Owning and dropping it prevents a leak fail-closed.
            drop(unsafe { OwnedFd::from_raw_fd(descriptor) });
            (11, 0, None)
        }
        _ if status == 0 => (11, 0, None),
        _ => (status, 0, None),
    };
    if result.0 == 0 && (result.1 == 0 || result.1 > buffer.len()) {
        drop(result.2);
        return (11, 0, None);
    }
    result
}

#[cfg(feature = "seqpacket")]
pub(super) fn send_with_fd(
    client: RawFd,
    packet: &[u8],
    descriptor: Option<BorrowedFd<'_>>,
) -> c_int {
    // SAFETY: the slice is readable for its reported length and remains alive;
    // `client` and the optional transfer descriptor remain borrowed and open.
    // The native boundary retains neither and does not consume ownership.
    match descriptor {
        Some(descriptor) => {
            let raw_descriptor = descriptor.as_raw_fd();
            unsafe {
                t1_seqpacket_send_with_fds(
                    client,
                    packet.as_ptr().cast(),
                    packet.len(),
                    &raw const raw_descriptor,
                    1,
                )
            }
        }
        None => unsafe {
            t1_seqpacket_send_with_fds(
                client,
                packet.as_ptr().cast(),
                packet.len(),
                std::ptr::null(),
                0,
            )
        },
    }
}

#[cfg(all(feature = "seqpacket", any(test, feature = "seqpacket-test-support")))]
pub(super) fn seqpacket_pair_for_test() -> std::io::Result<(OwnedFd, OwnedFd)> {
    const AF_UNIX: c_int = 1;
    const SOCK_SEQPACKET: c_int = 5;
    const SOCK_CLOEXEC: c_int = 0o2_000_000;
    const SOCK_NONBLOCK: c_int = 0o4_000;

    let mut pair = [-1; 2];
    // SAFETY: `pair` is writable for two C integers. Exact success transfers
    // two new local socket descriptors and failure transfers neither.
    if unsafe {
        c_socketpair(
            AF_UNIX,
            SOCK_SEQPACKET | SOCK_CLOEXEC | SOCK_NONBLOCK,
            0,
            pair.as_mut_ptr(),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful socketpair returned two distinct owned descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(pair[0]), OwnedFd::from_raw_fd(pair[1])) })
}

#[cfg(all(feature = "seqpacket", test))]
pub(super) fn descriptor_is_cloexec_for_test(descriptor: BorrowedFd<'_>) -> std::io::Result<bool> {
    const F_GETFD: c_int = 1;
    const FD_CLOEXEC: c_int = 1;

    // SAFETY: the descriptor remains borrowed and open; F_GETFD takes no
    // variadic argument and returns descriptor flags without retaining state.
    let flags = unsafe { c_fcntl(descriptor.as_raw_fd(), F_GETFD) };
    if flags < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(flags & FD_CLOEXEC != 0)
    }
}

#[cfg(feature = "touchbar-drm")]
pub(super) fn open_touchbar_drm() -> (c_int, *mut RawTouchBarDrm, RawTouchBarDrmGeometry) {
    let mut display = std::ptr::null_mut();
    let mut geometry = RawTouchBarDrmGeometry {
        width: 0,
        height: 0,
        stride: 0,
        byte_length: 0,
    };
    // SAFETY: both outputs remain writable for the complete call. The native
    // boundary returns one exclusively owned opaque handle only on success and
    // retains no pointer to the geometry output.
    let status = unsafe { t1_touchbar_drm_open(&raw mut display, &raw mut geometry) };
    (status, display, geometry)
}

#[cfg(feature = "touchbar-drm")]
pub(super) fn present_touchbar_drm(display: *mut RawTouchBarDrm, pixels: &[u8]) -> c_int {
    // SAFETY: `display` is the exclusively owned live native handle held by
    // the safe wrapper. The pixel slice remains readable for the complete call
    // and the native boundary retains no pointer.
    unsafe { t1_touchbar_drm_present(display, pixels.as_ptr(), pixels.len()) }
}

#[cfg(feature = "touchbar-drm")]
pub(super) fn present_touchbar_drm_rectangles(
    display: *mut RawTouchBarDrm,
    pixels: &[u8],
    rectangles: &[RawTouchBarDrmDamageRectangle],
) -> c_int {
    // SAFETY: `display` is the exclusively owned live native handle held by
    // the safe wrapper. Both slices remain readable for the complete call and
    // the native boundary retains neither pointer.
    unsafe {
        t1_touchbar_drm_present_rectangles(
            display,
            pixels.as_ptr(),
            pixels.len(),
            rectangles.as_ptr(),
            rectangles.len(),
        )
    }
}

#[cfg(feature = "touchbar-drm")]
pub(super) fn close_touchbar_drm(display: *mut RawTouchBarDrm) {
    // SAFETY: the safe wrapper calls this exactly once for its owned handle.
    unsafe { t1_touchbar_drm_close(display) };
}

#[cfg(feature = "touchbar-io")]
pub(super) fn open_touchbar_digitizer() -> (c_int, Option<OwnedFd>) {
    let mut descriptor = -1;
    // SAFETY: `descriptor` remains writable for the call. On success the C
    // boundary transfers one newly opened descriptor and retains no pointer.
    let status = unsafe { t1_touchbar_digitizer_open(&raw mut descriptor) };
    let owned = if status == 0 && descriptor >= 0 {
        // SAFETY: the C contract transfers a new descriptor only on success.
        Some(unsafe { OwnedFd::from_raw_fd(descriptor) })
    } else {
        None
    };
    (status, owned)
}

#[cfg(feature = "touchbar-io")]
pub(super) fn read_touchbar_digitizer(
    descriptor: RawFd,
    timeout_ms: u32,
    report: &mut [u8; 52],
) -> c_int {
    // SAFETY: the borrowed descriptor remains open and `report` is writable
    // for its exact native protocol length. The C boundary retains neither.
    unsafe { t1_touchbar_digitizer_read(descriptor, timeout_ms, report.as_mut_ptr()) }
}

#[cfg(feature = "touchbar-io")]
pub(super) fn open_touchbar_fn() -> (c_int, Option<OwnedFd>) {
    let mut descriptor = -1;
    // SAFETY: `descriptor` remains writable for the call. On success the C
    // boundary transfers one newly opened descriptor and retains no pointer.
    let status = unsafe { t1_touchbar_fn_open(&raw mut descriptor) };
    let owned = if status == 0 && descriptor >= 0 {
        // SAFETY: the C contract transfers a new descriptor only on success.
        Some(unsafe { OwnedFd::from_raw_fd(descriptor) })
    } else {
        None
    };
    (status, owned)
}

#[cfg(feature = "touchbar-io")]
pub(super) fn touchbar_fn_state(descriptor: RawFd) -> (c_int, bool) {
    let mut pressed = 0;
    // SAFETY: the borrowed descriptor remains open and `pressed` remains
    // writable for the call. The C boundary writes only zero or one on success.
    let status = unsafe { t1_touchbar_fn_state(descriptor, &raw mut pressed) };
    (status, pressed == 1)
}

#[cfg(feature = "touchbar-io")]
pub(super) fn read_touchbar_fn(
    descriptor: RawFd,
    edges: &mut [RawTouchBarFnEdge],
) -> (c_int, usize) {
    let mut edge_count = 0;
    // SAFETY: the borrowed descriptor remains open; the output slice and count
    // remain writable for the call. The native boundary retains no pointer.
    let status = unsafe {
        t1_touchbar_fn_read(
            descriptor,
            edges.as_mut_ptr(),
            edges.len(),
            &raw mut edge_count,
        )
    };
    (status, edge_count)
}

#[cfg(feature = "touchbar-io")]
pub(super) fn wait_touchbar_inputs(
    digitizer: RawFd,
    function: RawFd,
    timeout_ms: u32,
) -> (c_int, u32) {
    let mut ready = 0;
    // SAFETY: both borrowed descriptors remain open and `ready` remains
    // writable for the call. The native boundary retains no descriptor.
    let status =
        unsafe { t1_touchbar_wait_inputs(digitizer, function, timeout_ms, &raw mut ready) };
    (status, ready)
}

#[cfg(feature = "touchbar-io")]
pub(super) fn wait_touchbar_events(
    digitizer: RawFd,
    function: RawFd,
    client: RawFd,
    client_interest: u32,
    timeout_ms: u32,
) -> (c_int, u32) {
    let mut ready = 0;
    // SAFETY: all borrowed descriptors remain open and `ready` remains
    // writable for the call. The native boundary retains no descriptor.
    let status = unsafe {
        t1_touchbar_wait_events(
            digitizer,
            function,
            client,
            client_interest,
            timeout_ms,
            &raw mut ready,
        )
    };
    (status, ready)
}

#[cfg(feature = "touchbar-io")]
pub(super) fn create_touchbar_keyboard() -> (c_int, *mut RawTouchBarKeyboard) {
    let mut device = std::ptr::null_mut();
    // SAFETY: `device` remains writable for the call. On success the native
    // boundary transfers one exclusively owned opaque handle.
    let status = unsafe { t1_touchbar_uinput_create(&raw mut device) };
    (status, device)
}

#[cfg(feature = "touchbar-io")]
pub(super) fn tap_touchbar_key(device: *mut RawTouchBarKeyboard, key: c_int) -> c_int {
    // SAFETY: the safe wrapper supplies its exclusively owned live handle and
    // a discriminant from the native allowlist.
    unsafe { t1_touchbar_uinput_tap(device, key) }
}

#[cfg(feature = "touchbar-io")]
pub(super) fn release_touchbar_keys(device: *mut RawTouchBarKeyboard) -> c_int {
    // SAFETY: the safe wrapper supplies its exclusively owned live handle.
    unsafe { t1_touchbar_uinput_release_all(device) }
}

#[cfg(feature = "touchbar-io")]
pub(super) fn close_touchbar_keyboard(device: *mut RawTouchBarKeyboard) -> c_int {
    // SAFETY: the safe wrapper transfers its exclusively owned handle exactly
    // once. The native boundary releases, destroys, closes, and frees it.
    unsafe { t1_touchbar_uinput_close(device) }
}

#[cfg(feature = "touchbar-session")]
pub(super) fn create_touchbar_session_watch() -> (c_int, *mut RawTouchBarSessionWatch) {
    let mut watch = std::ptr::null_mut();
    // SAFETY: `watch` is writable for one pointer. On success the native
    // boundary transfers one exclusively owned opaque monitor handle.
    let status = unsafe { t1_touchbar_session_watch_create(&raw mut watch) };
    (status, watch)
}

#[cfg(feature = "touchbar-session")]
pub(super) fn touchbar_session_poll_source(
    watch: &NonNull<RawTouchBarSessionWatch>,
) -> (c_int, Option<BorrowedFd<'_>>, c_int, u64) {
    let mut descriptor = -1;
    let mut events = 0;
    let mut timeout_usec = u64::MAX;
    // SAFETY: the safe wrapper supplies its exclusively owned live handle and
    // all scalar outputs remain writable for the complete call.
    let status = unsafe {
        t1_touchbar_session_watch_poll_source(
            watch.as_ptr(),
            &raw mut descriptor,
            &raw mut events,
            &raw mut timeout_usec,
        )
    };
    let descriptor = if status == 0 && descriptor >= 0 {
        // SAFETY: successful native output borrows the monitor's live fd. Its
        // lifetime is tied to the opaque handle borrow supplied by the caller.
        Some(unsafe { BorrowedFd::borrow_raw(descriptor) })
    } else {
        None
    };
    if status == 0 && descriptor.is_none() {
        // SAFETY: the handle remains exclusively owned by the safe wrapper.
        // An impossible malformed success must revoke admission fail-closed.
        unsafe { t1_touchbar_session_watch_release(watch.as_ptr()) };
        return (3, None, 0, u64::MAX);
    }
    (status, descriptor, events, timeout_usec)
}

#[cfg(feature = "touchbar-session")]
pub(super) fn admit_touchbar_session(watch: *mut RawTouchBarSessionWatch, peer_uid: u32) -> c_int {
    // SAFETY: the safe wrapper supplies its exclusively owned live handle and
    // the UID comes from a kernel-derived peer credential owned by the caller.
    unsafe { t1_touchbar_session_watch_admit(watch, peer_uid) }
}

#[cfg(feature = "touchbar-session")]
pub(super) fn refresh_touchbar_session(watch: *mut RawTouchBarSessionWatch) -> c_int {
    // SAFETY: the safe wrapper supplies its exclusively owned live handle.
    unsafe { t1_touchbar_session_watch_refresh(watch) }
}

#[cfg(feature = "touchbar-session")]
pub(super) fn set_touchbar_session_brightness(
    watch: *mut RawTouchBarSessionWatch,
    subsystem: &CStr,
    name: &CStr,
    value: u32,
) -> c_int {
    // SAFETY: the safe wrapper supplies its exclusively owned admitted watch
    // and two live NUL-terminated strings. The native call retains neither.
    unsafe {
        t1_touchbar_session_watch_set_brightness(watch, subsystem.as_ptr(), name.as_ptr(), value)
    }
}

#[cfg(feature = "touchbar-session")]
pub(super) fn touchbar_session_is_admitted(watch: *const RawTouchBarSessionWatch) -> bool {
    // SAFETY: the safe wrapper supplies its live handle and the query retains
    // no pointer or other state.
    unsafe { t1_touchbar_session_watch_is_admitted(watch) == 1 }
}

#[cfg(feature = "touchbar-session")]
pub(super) fn release_touchbar_session(watch: *mut RawTouchBarSessionWatch) {
    // SAFETY: the safe wrapper supplies its exclusively owned live handle.
    unsafe { t1_touchbar_session_watch_release(watch) };
}

#[cfg(feature = "touchbar-session")]
pub(super) fn destroy_touchbar_session_watch(watch: *mut RawTouchBarSessionWatch) {
    // SAFETY: the safe owner transfers its unique handle exactly once on drop.
    unsafe { t1_touchbar_session_watch_destroy(watch) };
}

#[cfg(feature = "xz")]
pub(super) fn decode_xz(input: &[u8], output: &mut [u8], memory_limit: u64) -> (c_int, usize) {
    let mut output_size = 0;
    // SAFETY: both slices remain alive and disjoint for the call. The C API
    // accepts the output slice's dangling non-null pointer when its length is
    // zero and never retains either pointer.
    let status = unsafe {
        t1_xz_decode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
            memory_limit,
            &raw mut output_size,
        )
    };
    (status, output_size)
}

#[cfg(all(test, feature = "sep-operation"))]
mod keystore_observer_tests {
    use super::*;

    unsafe extern "C" fn sentinel(_: u8, _: c_int, _: i8, _: i32) {}

    #[test]
    fn observer_is_thread_local_and_restored_after_cancelled_relay() {
        // SAFETY: callbacks are static and scalar-only; no context is retained.
        let original = unsafe { sep_session_set_keystore_observer(Some(sentinel)) };
        let other_thread_was_empty = std::thread::spawn(|| {
            // SAFETY: reads/resets only this fresh thread's observer.
            unsafe { sep_session_set_keystore_observer(None).is_none() }
        })
        .join()
        .unwrap();
        let (status, ready) = run_sep_notification_relay(100, 25, &|| true, || true);
        // SAFETY: restore the observer that preceded this test.
        let restored = unsafe { sep_session_set_keystore_observer(original) };
        assert!(other_thread_was_empty);
        assert_eq!(status, -103);
        assert!(!ready);
        assert!(std::ptr::fn_addr_eq(
            restored.unwrap(),
            sentinel as RawKeystoreObserver
        ));
    }
}

#[cfg(all(test, feature = "sep-operation"))]
mod cleanup_observer_tests {
    use super::*;

    unsafe extern "C" fn sentinel(_: c_int) {}

    fn assert_sentinel_installed() {
        // SAFETY: inspect and restore this test thread's scalar-only callback.
        let current = unsafe { sep_operation_set_cleanup_observer(None) };
        unsafe { sep_operation_set_cleanup_observer(current) };
        assert!(std::ptr::fn_addr_eq(
            current.unwrap(),
            sentinel as RawSepCleanupObserver
        ));
    }

    #[test]
    fn every_native_owner_restores_the_thread_local_cleanup_observer() {
        // SAFETY: the hook is static and thread-local; cancellation prevents I/O.
        let original = unsafe { sep_operation_set_cleanup_observer(Some(sentinel)) };
        assert!(
            std::thread::spawn(|| unsafe { sep_operation_set_cleanup_observer(None).is_none() })
                .join()
                .unwrap()
        );
        let cancelled = AtomicBool::new(true);
        assert_eq!(run_sep_authorized(100, 1000, &cancelled, |_| ()).0, -103);
        assert_sentinel_installed();
        assert_eq!(run_sep_keybag(0, 0, 100, &cancelled, |_, _| ()).0, -103);
        assert_sentinel_installed();
        assert_eq!(
            run_sep_prepared_keybag(0, 0, 100, &cancelled, || Ok::<_, ()>(()), |(), _, _| ()).0,
            -103
        );
        assert_sentinel_installed();
        assert_eq!(
            run_sep_notification_relay(100, 25, &|| true, || true).0,
            -103
        );
        assert_sentinel_installed();
        unsafe { sep_operation_set_cleanup_observer(original) };
    }

    #[test]
    fn disabled_observation_and_unwinding_restore_nested_hooks() {
        // SAFETY: hooks are scalar-only, thread-local, and restored below.
        let original = unsafe { sep_operation_set_cleanup_observer(Some(sentinel)) };
        {
            let _disabled = SepCleanupObservation::install(false);
            assert!(unsafe { sep_operation_set_cleanup_observer(None).is_none() });
        }
        assert_sentinel_installed();
        let result = std::panic::catch_unwind(|| {
            let _enabled = SepCleanupObservation::install(true);
            let current = unsafe { sep_operation_set_cleanup_observer(None) };
            unsafe { sep_operation_set_cleanup_observer(current) };
            assert!(std::ptr::fn_addr_eq(
                current.unwrap(),
                observe_sep_cleanup as RawSepCleanupObserver
            ));
            panic!("synthetic callback unwind");
        });
        assert!(result.is_err());
        assert_sentinel_installed();
        unsafe { sep_operation_set_cleanup_observer(original) };
    }
}
