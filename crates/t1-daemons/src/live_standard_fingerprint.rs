#![cfg(feature = "auth-broker-service")]

//! Live composition for broker-authorized standard fingerprint operations.

use std::cell::RefCell;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use t1_bridge::control::{BridgeCommandError, ControlError, ensure_fdr_calibration_loaded};
use t1_bridge::enroll_workflow::EnrollmentError;
use t1_bridge::live_operation::{
    CallbackEventError, LiveBiometricError, LiveBridgeConnection, LiveBridgeInterrupt,
    LiveClientPreparationError, LiveConnectionError, LivePolicyRetryRuntime,
    PreparedLiveBridgeConnection, bridge_command_native_status, live_biometric_native_status,
};
use t1_bridge::match_workflow::{IdentityMatchOutcome, MatchOutcome, MatchWorkflowError};
use t1_bridge::policy::BiometricUserId;
use t1_bridge::policy_workflow::PolicyWorkflowError;
use t1_bridge::session::SessionError;
use t1_bridge::user_workflow::UserWorkflowError;
use t1_platform::sep::{
    PreparedKeybagLeaseOutcome, SepCancellation, with_prepared_enrollment_keybag,
    with_prepared_existing_keybag,
};

use crate::auth_feedback::apply_authentication_feedback;
use crate::auth_lifecycle::{AuthenticationLifecycleError, with_authentication_relay_handoff};
use crate::auth_operation::{
    AuthenticationOperationError, match_prepared_identities, restore_user_after_calibration,
};
use crate::auth_protocol::MAX_MATCH_TIMEOUT;
use crate::auth_session::{ActiveStandardOperation, StandardCompletion};
use crate::catacomb_restore::CatacombRestoreError;
use crate::catacomb_session::CatacombSessionError;
use crate::catacomb_store::CatacombPairStore;
use crate::catacomb_store::CatacombRecoveryError;
use crate::enrollment_lifecycle::{
    claim_enrollment_owner, run_enrollment_transaction_after_handoff, with_enrollment_relay_handoff,
};
use crate::enrollment_owner::{EnrollmentOwner, EnrollmentOwnerStore};
use crate::enrollment_transaction::standard_enrollment::{
    PreparedStandardEnrollment, StandardEnrollmentError, StandardEnrollmentPreparationError,
    prepare_standard_enrollment, run_reserved_standard_enrollment,
};
use crate::fingerprint_diagnostics::DiagnosticTransport;
use crate::identity_lifecycle::{IdentityRemovalCommandError, IdentityRemovalError};
use crate::keybag_relay::SystemctlKeybagRelay;
use crate::machine_data::{MachineCalibration, read_machine_calibration};
use crate::nss_account::resolve_standard_account;
use crate::overlay::{DEFAULT_STATE_PATH, OverlaySession, OverlayState};
use crate::request_ids::LinuxRequestIdSource;
use crate::sep_lifecycle::{KeybagRelayControl, SepEnrollmentLeaseRuntime, bootstrap_keybag};
use crate::standard_catalog_store::{
    load_committed_catalog_for_account, load_committed_standard_list,
};
use crate::standard_fingerprint_protocol::{
    EnrollProgress, FingerLabel, IdentityId, ServerMessage, TerminalOutcome,
};
use crate::standard_identity_catalog::StandardIdentityCatalog;
use crate::standard_identity_catalog::deletion::{
    StandardIdentityDeletionError, delete_prepared_labeled_identity_with_transaction,
};
use crate::standard_operation_authority::{ResolvedStandardAccount, ResolvedStandardOperation};
use crate::standard_query_policy::{
    StandardMatchOutcome, evaluate_identify, evaluate_verify, validate_verify_request,
};
use crate::xart_live::ValidatedNcmInterface;
use t1_platform::diagnostics::{self as diagnostics, Component, Stage as Phase};

const STATE_DIRECTORY: &str = "/var/lib/t1bridge/touch-id/catacombs";
const RELAY_CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const SEP_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const ENROLLMENT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const CANCELLATION_POLL: Duration = Duration::from_millis(25);

struct LiveEnvironment {
    calibration: MachineCalibration,
    interface: ValidatedNcmInterface,
}

#[derive(Clone, Default)]
struct StandardCancellation {
    sep: SepCancellation,
    bridge: Arc<Mutex<Option<LiveBridgeInterrupt>>>,
}

impl StandardCancellation {
    fn new() -> Self {
        Self::default()
    }

    fn sep(&self) -> &SepCancellation {
        &self.sep
    }

    fn is_cancelled(&self) -> bool {
        self.sep.is_cancelled()
    }

    fn cancel(&self) {
        self.sep.cancel();
        if let Ok(bridge) = self.bridge.lock()
            && let Some(bridge) = bridge.as_ref()
        {
            bridge.interrupt();
        }
    }

    fn register_bridge(
        &self,
        connection: &LiveBridgeConnection,
    ) -> Result<(), LiveStandardFailure> {
        let interrupt = connection
            .interrupt_handle()
            .map_err(|_| LiveStandardFailure::Error)?;
        let mut bridge = self.bridge.lock().map_err(|_| LiveStandardFailure::Error)?;
        if self.is_cancelled() {
            interrupt.interrupt();
            return Err(LiveStandardFailure::Cancelled);
        }
        *bridge = Some(interrupt);
        Ok(())
    }
}

struct PreparedLiveStandardEnrollment<'store> {
    connection: PreparedLiveBridgeConnection,
    enrollment: Option<PreparedStandardEnrollment<'store>>,
}

struct OverlayTeardown<'session>(&'session RefCell<Option<OverlaySession>>);

impl Drop for OverlayTeardown<'_> {
    fn drop(&mut self) {
        drop(self.0.borrow_mut().take());
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LiveStandardFailure {
    Cancelled,
    CapacityFull,
    DeviceLost,
    Duplicate,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiveMatchPass {
    Restored,
    Matched(IdentityMatchOutcome),
}

trait StandardOperationRuntime {
    fn list(&mut self, active: &ActiveStandardOperation) -> ServerMessage;

    fn enroll(
        &mut self,
        active: &ActiveStandardOperation,
        account: &ResolvedStandardAccount,
        finger: FingerLabel,
        progress: &mut dyn FnMut(EnrollProgress),
    ) -> ServerMessage;

    fn verify(
        &mut self,
        active: &ActiveStandardOperation,
        account: &ResolvedStandardAccount,
        identity: IdentityId,
    ) -> ServerMessage;

    fn identify(
        &mut self,
        active: &ActiveStandardOperation,
        account: &ResolvedStandardAccount,
    ) -> ServerMessage;

    fn delete(
        &mut self,
        active: &ActiveStandardOperation,
        account: &ResolvedStandardAccount,
        identity: IdentityId,
    ) -> ServerMessage;
}

struct LiveStandardRuntime;

/// Runs one standard operation without creating another broker coordinator.
///
/// The returned completion remains bound to the exact active token and shared
/// cancellation registration. Enrollment progress is emitted separately on
/// the caller-owned connection sink and is never accepted as a completion.
#[must_use]
pub fn run_live_standard_operation(
    active: &ActiveStandardOperation,
    progress: &mut dyn FnMut(EnrollProgress),
) -> StandardCompletion {
    run_with_runtime(active, progress, &mut LiveStandardRuntime)
}

fn run_with_runtime<Runtime: StandardOperationRuntime>(
    active: &ActiveStandardOperation,
    progress: &mut dyn FnMut(EnrollProgress),
    runtime: &mut Runtime,
) -> StandardCompletion {
    let result = if active.is_cancelled() {
        terminal(TerminalOutcome::Cancelled)
    } else {
        match active.operation().operation() {
            ResolvedStandardOperation::ListIdentities => runtime.list(active),
            ResolvedStandardOperation::Enroll { account, finger } => {
                runtime.enroll(active, account, *finger, progress)
            }
            ResolvedStandardOperation::Verify { account, identity } => {
                runtime.verify(active, account, *identity)
            }
            ResolvedStandardOperation::Identify { account } => runtime.identify(active, account),
            ResolvedStandardOperation::DeleteIdentity { account, identity } => {
                runtime.delete(active, account, *identity)
            }
        }
    };
    active.completion_for_worker(result)
}

impl StandardOperationRuntime for LiveStandardRuntime {
    fn list(&mut self, active: &ActiveStandardOperation) -> ServerMessage {
        if active.is_cancelled() {
            return terminal(TerminalOutcome::Cancelled);
        }
        let pair_store = CatacombPairStore::new(STATE_DIRECTORY);
        let owner_store = EnrollmentOwnerStore::new(STATE_DIRECTORY);
        t1_platform::diagnostics::observe(
            t1_platform::diagnostics::Component::Broker,
            Phase::List,
            || load_committed_standard_list(&pair_store, &owner_store),
        )
        .map_or_else(
            |_| terminal(TerminalOutcome::Error),
            |list| {
                if active.is_cancelled() {
                    terminal(TerminalOutcome::Cancelled)
                } else {
                    list.into_server_message()
                }
            },
        )
    }

    fn enroll(
        &mut self,
        active: &ActiveStandardOperation,
        account: &ResolvedStandardAccount,
        finger: FingerLabel,
        progress: &mut dyn FnMut(EnrollProgress),
    ) -> ServerMessage {
        let result = diagnostics::observe(Component::Broker, Phase::Enroll, || {
            run_live_enroll(active, account, finger, progress)
        });
        map_failure(result.map(|identity| terminal(TerminalOutcome::Enrolled(identity))))
    }

    fn verify(
        &mut self,
        active: &ActiveStandardOperation,
        account: &ResolvedStandardAccount,
        identity: IdentityId,
    ) -> ServerMessage {
        map_failure(
            t1_platform::diagnostics::observe(
                t1_platform::diagnostics::Component::Broker,
                Phase::Match,
                || run_live_query(active, account, Some(identity)),
            )
            .map(match_message),
        )
    }

    fn identify(
        &mut self,
        active: &ActiveStandardOperation,
        account: &ResolvedStandardAccount,
    ) -> ServerMessage {
        map_failure(
            t1_platform::diagnostics::observe(
                t1_platform::diagnostics::Component::Broker,
                Phase::Match,
                || run_live_query(active, account, None),
            )
            .map(match_message),
        )
    }

    fn delete(
        &mut self,
        active: &ActiveStandardOperation,
        account: &ResolvedStandardAccount,
        identity: IdentityId,
    ) -> ServerMessage {
        map_failure(
            t1_platform::diagnostics::observe(
                t1_platform::diagnostics::Component::Broker,
                Phase::Delete,
                || run_live_delete(active, account, identity),
            )
            .map(|()| terminal(TerminalOutcome::Completed)),
        )
    }
}

fn run_live_enroll(
    active: &ActiveStandardOperation,
    account: &ResolvedStandardAccount,
    finger: FingerLabel,
    progress: &mut dyn FnMut(EnrollProgress),
) -> Result<IdentityId, LiveStandardFailure> {
    let pair_store = CatacombPairStore::new(STATE_DIRECTORY);
    let owner_store = EnrollmentOwnerStore::new(STATE_DIRECTORY);
    revalidate_and_claim_owner(active, account, &pair_store, &owner_store)?;
    let environment = live_environment()?;
    let mut relay =
        SystemctlKeybagRelay::new(RELAY_CONTROL_TIMEOUT).map_err(|_| LiveStandardFailure::Error)?;

    let result = with_standard_cancellation(active, environment.interface, |cancellation| {
        diagnostics::observe(Component::Broker, Phase::Relay, || {
            prepare_enrollment_relay(&mut relay, || {
                bootstrap_enrollment_keybag(cancellation.sep())
            })
        })?;
        if standard_operation_cancelled(active, cancellation) {
            return Err(LiveStandardFailure::Cancelled);
        }
        with_enrollment_relay_handoff(&mut relay, |_| {
            let user_id = biometric_user(active)?;
            let mut lease =
                SepEnrollmentLeaseRuntime::new(SEP_OPERATION_TIMEOUT, cancellation.sep().clone());
            run_enrollment_transaction_after_handoff(
                &mut lease,
                || {
                    if standard_operation_cancelled(active, cancellation) {
                        return Err(LiveStandardFailure::Cancelled);
                    }
                    let connection =
                        LiveBridgeConnection::connect(environment.interface.kernel_index())
                            .map_err(|error| map_live_connection_error(&error))?;
                    cancellation.register_bridge(&connection)?;
                    let request_ids =
                        LinuxRequestIdSource::open().map_err(|_| LiveStandardFailure::Error)?;
                    let mut enrollment = None;
                    let connection = connection
                        .prepare(request_ids, |transport| {
                            let mut transport = DiagnosticTransport::new(
                                transport,
                                Phase::Preparation,
                                bridge_command_native_status,
                            );
                            enrollment = Some(
                                prepare_standard_enrollment(
                                    &mut transport,
                                    &pair_store,
                                    user_id,
                                    environment.calibration.as_bytes(),
                                )
                                .map_err(|error| {
                                    map_standard_enrollment_preparation_error(&error)
                                })?,
                            );
                            Ok::<(), LiveStandardFailure>(())
                        })
                        .map_err(map_preparation_error)?;
                    Ok(PreparedLiveStandardEnrollment {
                        connection,
                        enrollment,
                    })
                },
                |prepared, credential| {
                    run_prepared_enrollment(
                        active,
                        cancellation,
                        prepared,
                        credential,
                        user_id,
                        account,
                        finger,
                        progress,
                    )
                },
            )
            .map_err(|error| match error {
                crate::enrollment_lifecycle::EnrollmentLifecycleError::Operation(
                    crate::enrollment_lifecycle::EnrollmentOperationError::Preparation(error)
                    | crate::enrollment_lifecycle::EnrollmentOperationError::Transaction(error),
                ) => error,
                _ if standard_operation_cancelled(active, cancellation) => {
                    LiveStandardFailure::Cancelled
                }
                _ => LiveStandardFailure::Error,
            })
        })
        .map_err(|error| match error {
            crate::enrollment_lifecycle::EnrollmentLifecycleError::Operation(error) => error,
            _ if standard_operation_cancelled(active, cancellation) => {
                LiveStandardFailure::Cancelled
            }
            _ => LiveStandardFailure::Error,
        })
    })?;
    match result {
        Err(_) if active.is_cancelled() => Err(LiveStandardFailure::Cancelled),
        result => result,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_prepared_enrollment(
    active: &ActiveStandardOperation,
    cancellation: &StandardCancellation,
    prepared: &mut PreparedLiveStandardEnrollment<'_>,
    credential: &[u8],
    user_id: BiometricUserId,
    account: &ResolvedStandardAccount,
    finger: FingerLabel,
    progress: &mut dyn FnMut(EnrollProgress),
) -> Result<IdentityId, LiveStandardFailure> {
    let request_ids = LinuxRequestIdSource::open().map_err(|_| LiveStandardFailure::Error)?;
    let operation = prepared
        .connection
        .start_operation(request_ids)
        .map_err(|error| map_live_connection_error(&error))?;
    let (mut transport, mut events) = operation
        .into_adapters(ENROLLMENT_TIMEOUT, || {
            standard_operation_cancelled(active, cancellation)
        })
        .map_err(|_| LiveStandardFailure::Error)?;
    let enrollment = prepared
        .enrollment
        .take()
        .ok_or(LiveStandardFailure::Error)?;
    let mut retry_runtime = LivePolicyRetryRuntime::new();
    let overlay = RefCell::new(None);
    let _teardown = OverlayTeardown(&overlay);
    let mut before_start = || {
        *overlay.borrow_mut() =
            OverlaySession::activate_optional(DEFAULT_STATE_PATH, OverlayState::Enrollment);
    };
    let mut report = |event: EnrollProgress| {
        if let Some(overlay) = overlay.borrow().as_ref() {
            overlay.update_standard_progress(event.completed_stage(), event.total_stages());
        }
        progress(event);
    };
    let mut transport = DiagnosticTransport::new(
        &mut transport,
        Phase::Transaction,
        live_biometric_native_status,
    );
    match run_reserved_standard_enrollment(
        &mut transport,
        &mut retry_runtime,
        &mut events,
        enrollment.transaction,
        user_id,
        account.canonical_username().clone(),
        finger,
        enrollment.committed_metadata,
        enrollment.active_pair,
        credential,
        Some(&mut report),
        &mut match_prepared_identities,
        &mut before_start,
        &mut || close_standard_operation_cancellation(active, cancellation),
    ) {
        Ok(success) => Ok(success.identity),
        Err(StandardEnrollmentError::IdentityCapacityReached) => {
            Err(LiveStandardFailure::CapacityFull)
        }
        Err(StandardEnrollmentError::DuplicateIdentity) => Err(LiveStandardFailure::Duplicate),
        Err(StandardEnrollmentError::EnrollmentTimedOut) => Err(LiveStandardFailure::Error),
        Err(_) if standard_operation_cancelled(active, cancellation) => {
            Err(LiveStandardFailure::Cancelled)
        }
        Err(error) => {
            eprintln!("t1bridge standard enrollment: result={error:?}");
            Err(map_standard_enrollment_error(&error))
        }
    }
}

fn run_live_query(
    active: &ActiveStandardOperation,
    account: &ResolvedStandardAccount,
    requested: Option<IdentityId>,
) -> Result<StandardMatchOutcome, LiveStandardFailure> {
    let pair_store = CatacombPairStore::new(STATE_DIRECTORY);
    let owner_store = EnrollmentOwnerStore::new(STATE_DIRECTORY);
    let catalog = load_committed_catalog_for_account(&pair_store, &owner_store, account)
        .map_err(|_| LiveStandardFailure::Error)?;
    if let Some(identity) = requested {
        validate_verify_request(&catalog, identity).map_err(|_| LiveStandardFailure::Error)?;
    }
    let overlay = RefCell::new(OverlaySession::activate_optional(
        DEFAULT_STATE_PATH,
        OverlayState::Authenticate,
    ));
    let _teardown = OverlayTeardown(&overlay);
    let outcome = run_live_match(active, &pair_store)?;
    let outcome = match requested {
        Some(identity) => {
            evaluate_verify(&catalog, identity, outcome).map_err(|_| LiveStandardFailure::Error)?
        }
        None => evaluate_identify(&catalog, outcome),
    };
    let mut overlay = overlay.borrow_mut();
    let _: Result<MatchOutcome, Infallible> =
        apply_authentication_feedback(Ok(standard_match(outcome)), overlay.as_mut());
    Ok(outcome)
}

fn run_live_match(
    active: &ActiveStandardOperation,
    pair_store: &CatacombPairStore,
) -> Result<IdentityMatchOutcome, LiveStandardFailure> {
    let environment = live_environment()?;
    let mut relay =
        SystemctlKeybagRelay::new(RELAY_CONTROL_TIMEOUT).map_err(|_| LiveStandardFailure::Error)?;
    let result = with_standard_cancellation(active, environment.interface, |cancellation| {
        complete_match_after_restore(|| {
            run_live_match_pass(active, pair_store, &environment, &mut relay, cancellation)
        })
    })?;
    match result {
        Err(_) if active.is_cancelled() => Err(LiveStandardFailure::Cancelled),
        result => result,
    }
}

fn run_live_match_pass(
    active: &ActiveStandardOperation,
    pair_store: &CatacombPairStore,
    environment: &LiveEnvironment,
    relay: &mut SystemctlKeybagRelay,
    cancellation: &StandardCancellation,
) -> Result<LiveMatchPass, LiveStandardFailure> {
    with_authentication_relay_handoff(relay, |_| {
        if standard_operation_cancelled(active, cancellation) {
            return Err(LiveStandardFailure::Cancelled);
        }
        let outcome = with_prepared_existing_keybag(
            SEP_OPERATION_TIMEOUT,
            cancellation.sep(),
            || prepare_live_connection(active, environment, cancellation),
            |prepared, credential| {
                run_prepared_match(
                    active,
                    cancellation,
                    prepared,
                    credential.as_bytes(),
                    pair_store,
                )
            },
        );
        match outcome {
            PreparedKeybagLeaseOutcome::Completed { operation, .. } => operation,
            PreparedKeybagLeaseOutcome::PreparationFailed(error) => Err(error),
            PreparedKeybagLeaseOutcome::AcquisitionFailed(_)
            | PreparedKeybagLeaseOutcome::CleanupFailed { .. }
                if standard_operation_cancelled(active, cancellation) =>
            {
                Err(LiveStandardFailure::Cancelled)
            }
            PreparedKeybagLeaseOutcome::AcquisitionFailed(_)
            | PreparedKeybagLeaseOutcome::CleanupFailed { .. } => Err(LiveStandardFailure::Error),
        }
    })
    .map_err(|error| match error {
        AuthenticationLifecycleError::Operation(error) => error,
        _ if standard_operation_cancelled(active, cancellation) => LiveStandardFailure::Cancelled,
        _ => LiveStandardFailure::Error,
    })
}

fn complete_match_after_restore(
    mut run_pass: impl FnMut() -> Result<LiveMatchPass, LiveStandardFailure>,
) -> Result<IdentityMatchOutcome, LiveStandardFailure> {
    match run_pass()? {
        LiveMatchPass::Matched(outcome) => Ok(outcome),
        LiveMatchPass::Restored => match run_pass()? {
            LiveMatchPass::Matched(outcome) => Ok(outcome),
            LiveMatchPass::Restored => Err(LiveStandardFailure::Error),
        },
    }
}

fn run_prepared_match(
    active: &ActiveStandardOperation,
    cancellation: &StandardCancellation,
    prepared: &mut PreparedLiveBridgeConnection,
    credential: &[u8],
    pair_store: &CatacombPairStore,
) -> Result<LiveMatchPass, LiveStandardFailure> {
    let request_ids = LinuxRequestIdSource::open().map_err(|_| LiveStandardFailure::Error)?;
    let operation = prepared
        .start_operation(request_ids)
        .map_err(|error| map_live_connection_error(&error))?;
    let (mut transport, mut events) = operation
        .into_adapters(MAX_MATCH_TIMEOUT, || {
            standard_operation_cancelled(active, cancellation)
        })
        .map_err(|_| LiveStandardFailure::Error)?;
    let mut transport =
        DiagnosticTransport::new(&mut transport, Phase::Match, live_biometric_native_status);
    let mut retry_runtime = LivePolicyRetryRuntime::new();
    let prepared_user = restore_user_after_calibration::<_, _, CallbackEventError>(
        &mut transport,
        &mut retry_runtime,
        biometric_user(active)?,
        Some(pair_store),
        Some(credential),
    )
    .map_err(|error| {
        if standard_operation_cancelled(active, cancellation) {
            return LiveStandardFailure::Cancelled;
        }
        let restore_native_status = match &error {
            AuthenticationOperationError::Restore(CatacombRestoreError::Transport {
                error,
                ..
            }) => live_biometric_native_status(error),
            _ => None,
        };
        eprintln!(
            "t1bridge standard fingerprint: authentication failed: {error:?}; \
             restore-native-status={restore_native_status:?}"
        );
        map_authentication_operation_error(&error)
    })?;
    if prepared_user.restored_in_this_lease() {
        return Ok(LiveMatchPass::Restored);
    }

    let outcome = match_prepared_identities(
        &mut transport,
        &mut events,
        biometric_user(active)?,
        prepared_user.identities(),
    )
    .map_err(|error| {
        if standard_operation_cancelled(active, cancellation) {
            LiveStandardFailure::Cancelled
        } else {
            map_authentication_operation_error(&AuthenticationOperationError::Match(error))
        }
    })?;
    Ok(LiveMatchPass::Matched(outcome))
}

fn run_live_delete(
    active: &ActiveStandardOperation,
    account: &ResolvedStandardAccount,
    identity: IdentityId,
) -> Result<(), LiveStandardFailure> {
    let pair_store = CatacombPairStore::new(STATE_DIRECTORY);
    let owner_store = EnrollmentOwnerStore::new(STATE_DIRECTORY);
    let catalog =
        load_committed_catalog_for_account(&pair_store, &owner_store, account).map_err(|_| {
            eprintln!("t1bridge standard deletion: stage=catalog result=error");
            LiveStandardFailure::Error
        })?;
    if !catalog.contains_labeled(identity) {
        eprintln!("t1bridge standard deletion: stage=target result=error");
        return Err(LiveStandardFailure::Error);
    }
    let environment = live_environment().inspect_err(|_| {
        eprintln!("t1bridge standard deletion: stage=environment result=error");
    })?;
    let mut relay =
        SystemctlKeybagRelay::new(RELAY_CONTROL_TIMEOUT).map_err(|_| LiveStandardFailure::Error)?;
    let result = with_standard_cancellation(active, environment.interface, |cancellation| {
        with_authentication_relay_handoff(&mut relay, |_| {
            if standard_operation_cancelled(active, cancellation) {
                return Err(LiveStandardFailure::Cancelled);
            }
            let outcome = with_prepared_enrollment_keybag(
                SEP_OPERATION_TIMEOUT,
                cancellation.sep(),
                || prepare_live_connection(active, &environment, cancellation),
                |prepared, credential| {
                    run_prepared_delete(
                        active,
                        cancellation,
                        prepared,
                        credential.as_bytes(),
                        &pair_store,
                        &catalog,
                        identity,
                    )
                },
            );
            match outcome {
                PreparedKeybagLeaseOutcome::Completed { operation, .. } => operation,
                PreparedKeybagLeaseOutcome::PreparationFailed(error) => {
                    eprintln!("t1bridge standard deletion: stage=lease-preparation result=error");
                    Err(error)
                }
                PreparedKeybagLeaseOutcome::AcquisitionFailed(_)
                | PreparedKeybagLeaseOutcome::CleanupFailed { .. }
                    if standard_operation_cancelled(active, cancellation) =>
                {
                    Err(LiveStandardFailure::Cancelled)
                }
                PreparedKeybagLeaseOutcome::AcquisitionFailed(_)
                | PreparedKeybagLeaseOutcome::CleanupFailed { .. } => {
                    eprintln!("t1bridge standard deletion: stage=lease result=error");
                    Err(LiveStandardFailure::Error)
                }
            }
        })
        .map_err(|error| match error {
            AuthenticationLifecycleError::Operation(error) => error,
            _ if standard_operation_cancelled(active, cancellation) => {
                LiveStandardFailure::Cancelled
            }
            _ => LiveStandardFailure::Error,
        })
    })?;
    match result {
        Err(_) if active.is_cancelled() => Err(LiveStandardFailure::Cancelled),
        result => result,
    }
}

fn run_prepared_delete(
    active: &ActiveStandardOperation,
    cancellation: &StandardCancellation,
    prepared: &mut PreparedLiveBridgeConnection,
    credential: &[u8],
    pair_store: &CatacombPairStore,
    catalog: &StandardIdentityCatalog,
    identity: IdentityId,
) -> Result<(), LiveStandardFailure> {
    let request_ids = LinuxRequestIdSource::open().map_err(|_| LiveStandardFailure::Error)?;
    let operation = prepared
        .start_operation(request_ids)
        .map_err(|error| map_live_connection_error(&error))?;
    let (mut transport, _events) = operation
        .into_adapters(MAX_MATCH_TIMEOUT, || {
            standard_operation_cancelled(active, cancellation)
        })
        .map_err(|_| LiveStandardFailure::Error)?;
    let user_id = biometric_user(active)?;
    let mut transport =
        DiagnosticTransport::new(&mut transport, Phase::Delete, live_biometric_native_status);
    let mut retry_runtime = LivePolicyRetryRuntime::new();
    let restored = restore_user_after_calibration::<_, _, Infallible>(
        &mut transport,
        &mut retry_runtime,
        user_id,
        Some(pair_store),
        Some(credential),
    )
    .map_err(|error| {
        eprintln!("t1bridge standard deletion: stage=restore result=error detail={error:?}");
        if standard_operation_cancelled(active, cancellation) {
            LiveStandardFailure::Cancelled
        } else {
            map_authentication_operation_error(&error)
        }
    })?;
    if !close_standard_operation_cancellation(active, cancellation) {
        return Err(LiveStandardFailure::Cancelled);
    }
    let transaction = pair_store.begin_transaction().map_err(|_| {
        eprintln!("t1bridge standard deletion: stage=transaction result=error");
        LiveStandardFailure::Error
    })?;
    delete_prepared_labeled_identity_with_transaction(
        &mut transport,
        transaction,
        user_id,
        restored.identities(),
        catalog,
        identity,
    )
    .map_err(|error| {
        eprintln!(
            "t1bridge standard deletion: stage=identity-removal result=error detail={error:?}"
        );
        map_standard_deletion_error(&error)
    })?;
    Ok(())
}

fn prepare_live_connection(
    active: &ActiveStandardOperation,
    environment: &LiveEnvironment,
    cancellation: &StandardCancellation,
) -> Result<PreparedLiveBridgeConnection, LiveStandardFailure> {
    if standard_operation_cancelled(active, cancellation) {
        return Err(LiveStandardFailure::Cancelled);
    }
    let connection = LiveBridgeConnection::connect(environment.interface.kernel_index())
        .map_err(|error| map_live_connection_error(&error))?;
    cancellation.register_bridge(&connection)?;
    let request_ids = LinuxRequestIdSource::open().map_err(|_| LiveStandardFailure::Error)?;
    connection
        .prepare(request_ids, |transport| {
            let mut transport = DiagnosticTransport::new(
                transport,
                Phase::Preparation,
                bridge_command_native_status,
            );
            ensure_fdr_calibration_loaded(&mut transport, environment.calibration.as_bytes())
                .map(|_| ())
                .map_err(|_| LiveStandardFailure::Error)
        })
        .map_err(map_preparation_error)
}

fn live_environment() -> Result<LiveEnvironment, LiveStandardFailure> {
    Ok(LiveEnvironment {
        calibration: read_machine_calibration().map_err(|_| LiveStandardFailure::Error)?,
        interface: ValidatedNcmInterface::discover()
            .map_err(|_| LiveStandardFailure::DeviceLost)?,
    })
}

fn revalidate_and_claim_owner(
    active: &ActiveStandardOperation,
    account: &ResolvedStandardAccount,
    pair_store: &CatacombPairStore,
    owner_store: &EnrollmentOwnerStore,
) -> Result<(), LiveStandardFailure> {
    let current = resolve_standard_account(account.canonical_username())
        .map_err(|_| LiveStandardFailure::Error)?;
    if current != *account || active.operation().target_user_id() != Some(current.user_id()) {
        return Err(LiveStandardFailure::Error);
    }
    let owner = EnrollmentOwner::new(current.user_id()).map_err(|_| LiveStandardFailure::Error)?;
    claim_enrollment_owner(pair_store, owner_store, owner).map_err(|_| LiveStandardFailure::Error)
}

fn bootstrap_enrollment_keybag(cancellation: &SepCancellation) -> Result<(), LiveStandardFailure> {
    bootstrap_keybag(SEP_OPERATION_TIMEOUT, cancellation)
        .map(|_| ())
        .map_err(|_| {
            if cancellation.is_cancelled() {
                LiveStandardFailure::Cancelled
            } else {
                LiveStandardFailure::Error
            }
        })
}

pub(crate) fn prepare_enrollment_relay(
    relay: &mut impl KeybagRelayControl,
    bootstrap: impl FnOnce() -> Result<(), LiveStandardFailure>,
) -> Result<(), LiveStandardFailure> {
    if relay.is_active().map_err(|_| LiveStandardFailure::Error)? {
        return Ok(());
    }
    bootstrap()?;
    relay.start().map_err(|_| LiveStandardFailure::Error)?;
    if relay.is_active().map_err(|_| LiveStandardFailure::Error)? {
        Ok(())
    } else {
        Err(LiveStandardFailure::Error)
    }
}

fn biometric_user(
    active: &ActiveStandardOperation,
) -> Result<BiometricUserId, LiveStandardFailure> {
    BiometricUserId::new(i64::from(active.operation().biometric_user_id()))
        .map_err(|_| LiveStandardFailure::Error)
}

fn map_preparation_error<E>(error: LiveClientPreparationError<E>) -> LiveStandardFailure
where
    E: Into<LiveStandardFailure>,
{
    match error {
        LiveClientPreparationError::Connection(error) => map_live_connection_error(&error),
        LiveClientPreparationError::Preparation(error) => error.into(),
    }
}

fn map_live_connection_error(error: &LiveConnectionError) -> LiveStandardFailure {
    match error {
        LiveConnectionError::Socket => LiveStandardFailure::DeviceLost,
        LiveConnectionError::TimeoutConfiguration
        | LiveConnectionError::Session
        | LiveConnectionError::DescriptorDuplication
        | LiveConnectionError::ClientVersion => LiveStandardFailure::Error,
    }
}

fn bridge_command_is_device_loss(error: &BridgeCommandError) -> bool {
    matches!(
        error,
        BridgeCommandError::Session(SessionError::Transport(_))
    )
}

fn live_transport_is_device_loss(error: &LiveBiometricError) -> bool {
    matches!(error, LiveBiometricError::Command(error) if bridge_command_is_device_loss(error))
}

fn control_is_device_loss(error: &ControlError<LiveBiometricError>) -> bool {
    matches!(error, ControlError::Transport(error) if live_transport_is_device_loss(error))
}

fn user_workflow_is_device_loss(error: &UserWorkflowError<LiveBiometricError>) -> bool {
    matches!(
        error,
        UserWorkflowError::Transport(error) if live_transport_is_device_loss(error)
    )
}

fn policy_is_device_loss(error: &PolicyWorkflowError<LiveBiometricError, Infallible>) -> bool {
    matches!(
        error,
        PolicyWorkflowError::Transport { error, .. } if live_transport_is_device_loss(error)
    )
}

fn catacomb_restore_is_device_loss(error: &CatacombRestoreError<LiveBiometricError>) -> bool {
    match error {
        CatacombRestoreError::Transport { error, .. } => live_transport_is_device_loss(error),
        CatacombRestoreError::Calibration(error) => control_is_device_loss(error),
        CatacombRestoreError::Store(_)
        | CatacombRestoreError::Command(_)
        | CatacombRestoreError::Response(_)
        | CatacombRestoreError::Catacomb(_)
        | CatacombRestoreError::Mesa(_)
        | CatacombRestoreError::LiveIdentityForAnotherUser
        | CatacombRestoreError::MasterNotLoaded { .. }
        | CatacombRestoreError::UserNotSecurelyLoaded { .. }
        | CatacombRestoreError::NoEnrolledFingerprints
        | CatacombRestoreError::RestoredIdentityForAnotherUser
        | CatacombRestoreError::CorruptedCatacomb => false,
    }
}

fn match_is_device_loss<EventError>(
    error: &MatchWorkflowError<LiveBiometricError, EventError>,
) -> bool {
    match error {
        MatchWorkflowError::Transport(error) | MatchWorkflowError::CleanupTransport(error) => {
            live_transport_is_device_loss(error)
        }
        MatchWorkflowError::NoEnrolledIdentities
        | MatchWorkflowError::IdentityForWrongUser
        | MatchWorkflowError::Command(_)
        | MatchWorkflowError::Mesa(_)
        | MatchWorkflowError::Event(_)
        | MatchWorkflowError::CleanupCommand(_) => false,
    }
}

fn authentication_is_device_loss<EventError>(
    error: &AuthenticationOperationError<LiveBiometricError, Infallible, EventError>,
) -> bool {
    match error {
        AuthenticationOperationError::Calibration(error) => control_is_device_loss(error),
        AuthenticationOperationError::Policy(error) => policy_is_device_loss(error),
        AuthenticationOperationError::Restore(error) => catacomb_restore_is_device_loss(error),
        AuthenticationOperationError::User(error) => user_workflow_is_device_loss(error),
        AuthenticationOperationError::Match(error) => match_is_device_loss(error),
        AuthenticationOperationError::MissingCredential
        | AuthenticationOperationError::Credential(_)
        | AuthenticationOperationError::Store(_) => false,
    }
}

fn map_authentication_operation_error<EventError>(
    error: &AuthenticationOperationError<LiveBiometricError, Infallible, EventError>,
) -> LiveStandardFailure {
    if authentication_is_device_loss(error) {
        LiveStandardFailure::DeviceLost
    } else {
        LiveStandardFailure::Error
    }
}

fn catacomb_session_is_device_loss(error: &CatacombSessionError<LiveBiometricError>) -> bool {
    matches!(
        error,
        CatacombSessionError::Transport(error) if live_transport_is_device_loss(error)
    )
}

fn enrollment_is_device_loss(
    error: &EnrollmentError<LiveBiometricError, CallbackEventError>,
) -> bool {
    matches!(
        error,
        EnrollmentError::Transport { error, .. } if live_transport_is_device_loss(error)
    )
}

fn bridge_control_is_device_loss(error: &ControlError<BridgeCommandError>) -> bool {
    matches!(error, ControlError::Transport(error) if bridge_command_is_device_loss(error))
}

fn bridge_catacomb_restore_is_device_loss(
    error: &CatacombRestoreError<BridgeCommandError>,
) -> bool {
    match error {
        CatacombRestoreError::Transport { error, .. } => bridge_command_is_device_loss(error),
        CatacombRestoreError::Calibration(error) => bridge_control_is_device_loss(error),
        CatacombRestoreError::Store(_)
        | CatacombRestoreError::Command(_)
        | CatacombRestoreError::Response(_)
        | CatacombRestoreError::Catacomb(_)
        | CatacombRestoreError::Mesa(_)
        | CatacombRestoreError::LiveIdentityForAnotherUser
        | CatacombRestoreError::MasterNotLoaded { .. }
        | CatacombRestoreError::UserNotSecurelyLoaded { .. }
        | CatacombRestoreError::NoEnrolledFingerprints
        | CatacombRestoreError::RestoredIdentityForAnotherUser
        | CatacombRestoreError::CorruptedCatacomb => false,
    }
}

fn map_standard_enrollment_preparation_error(
    error: &StandardEnrollmentPreparationError<BridgeCommandError>,
) -> LiveStandardFailure {
    let device_lost = match error {
        StandardEnrollmentPreparationError::Recovery(CatacombRecoveryError::Validator(error)) => {
            bridge_catacomb_restore_is_device_loss(error)
        }
        StandardEnrollmentPreparationError::Calibration(error) => {
            bridge_control_is_device_loss(error)
        }
        StandardEnrollmentPreparationError::Recovery(CatacombRecoveryError::Store(_))
        | StandardEnrollmentPreparationError::RecoveryBlocked => false,
        StandardEnrollmentPreparationError::Store(error) => {
            let _ = error;
            false
        }
        StandardEnrollmentPreparationError::Metadata(error) => {
            let _ = error;
            false
        }
    };
    if device_lost {
        LiveStandardFailure::DeviceLost
    } else {
        LiveStandardFailure::Error
    }
}

fn map_standard_enrollment_error(
    error: &StandardEnrollmentError<LiveBiometricError, Infallible, CallbackEventError>,
) -> LiveStandardFailure {
    let device_lost = match error {
        StandardEnrollmentError::Restore(error) => authentication_is_device_loss(error),
        StandardEnrollmentError::User(error)
        | StandardEnrollmentError::PostCommitIdentityVerification(error) => {
            user_workflow_is_device_loss(error)
        }
        StandardEnrollmentError::SystemPolicy(error)
        | StandardEnrollmentError::UserPolicy(error) => policy_is_device_loss(error),
        StandardEnrollmentError::Enrollment(error) => enrollment_is_device_loss(error),
        StandardEnrollmentError::DuplicateCheck(error) => match_is_device_loss(error),
        StandardEnrollmentError::Persistence(error) => catacomb_session_is_device_loss(error),
        StandardEnrollmentError::CatacombNotSecurelyLoaded
        | StandardEnrollmentError::IdentityCapacityReached
        | StandardEnrollmentError::DuplicateIdentity
        | StandardEnrollmentError::InvalidIdentity
        | StandardEnrollmentError::EnrollmentTimedOut
        | StandardEnrollmentError::EnrollmentCancelled
        | StandardEnrollmentError::PostCommitIdentitySetChanged => false,
        StandardEnrollmentError::Catalog(error) => {
            let _ = error;
            false
        }
    };
    if device_lost {
        LiveStandardFailure::DeviceLost
    } else {
        LiveStandardFailure::Error
    }
}

fn identity_removal_is_device_loss(error: &IdentityRemovalError<LiveBiometricError>) -> bool {
    match error {
        IdentityRemovalError::Preparation(error)
        | IdentityRemovalError::PostCommitVerification(error) => {
            user_workflow_is_device_loss(error)
        }
        IdentityRemovalError::CommandAmbiguous(IdentityRemovalCommandError::Transport(error)) => {
            live_transport_is_device_loss(error)
        }
        IdentityRemovalError::Persistence(error) => catacomb_session_is_device_loss(error),
        IdentityRemovalError::TargetMissing
        | IdentityRemovalError::PreMutationIdentityChange { .. }
        | IdentityRemovalError::CommandAmbiguous(IdentityRemovalCommandError::Response(_))
        | IdentityRemovalError::TargetStillPresent
        | IdentityRemovalError::CollateralIdentityChange { .. } => false,
    }
}

fn map_standard_deletion_error(
    error: &StandardIdentityDeletionError<LiveBiometricError>,
) -> LiveStandardFailure {
    match error {
        StandardIdentityDeletionError::Removal(error) if identity_removal_is_device_loss(error) => {
            LiveStandardFailure::DeviceLost
        }
        StandardIdentityDeletionError::Catalog(_) | StandardIdentityDeletionError::Removal(_) => {
            LiveStandardFailure::Error
        }
    }
}

fn with_standard_cancellation<T>(
    active: &ActiveStandardOperation,
    expected_interface: ValidatedNcmInterface,
    operation: impl FnOnce(&StandardCancellation) -> T,
) -> Result<T, LiveStandardFailure> {
    with_standard_cancellation_monitor(
        active,
        move || {
            ValidatedNcmInterface::discover().is_ok_and(|current| current == expected_interface)
        },
        operation,
    )
}

fn standard_operation_cancelled(
    active: &ActiveStandardOperation,
    cancellation: &StandardCancellation,
) -> bool {
    active.is_cancelled() || cancellation.is_cancelled()
}

fn close_standard_operation_cancellation(
    active: &ActiveStandardOperation,
    cancellation: &StandardCancellation,
) -> bool {
    !cancellation.is_cancelled() && active.close_cancellation() && !cancellation.is_cancelled()
}

fn with_standard_cancellation_monitor<T>(
    active: &ActiveStandardOperation,
    mut device_present: impl FnMut() -> bool + Send,
    operation: impl FnOnce(&StandardCancellation) -> T,
) -> Result<T, LiveStandardFailure> {
    const MONITOR_IDLE: u8 = 0;
    const MONITOR_CANCELLED: u8 = 1;
    const MONITOR_DEVICE_LOST: u8 = 2;

    let cancellation = StandardCancellation::new();
    let finished = Arc::new(AtomicBool::new(false));
    let reason = Arc::new(std::sync::atomic::AtomicU8::new(MONITOR_IDLE));
    thread::scope(|scope| {
        let monitor_finished = Arc::clone(&finished);
        let monitor_cancellation = cancellation.clone();
        let monitor_reason = Arc::clone(&reason);
        let monitor = scope.spawn(move || {
            while !monitor_finished.load(Ordering::Acquire) {
                if active.is_cancelled() {
                    monitor_reason.store(MONITOR_CANCELLED, Ordering::Release);
                    monitor_cancellation.cancel();
                    break;
                }
                if !device_present() {
                    eprintln!("t1bridge standard fingerprint: device-presence=lost action=cancel");
                    monitor_reason.store(MONITOR_DEVICE_LOST, Ordering::Release);
                    monitor_cancellation.cancel();
                    break;
                }
                thread::park_timeout(CANCELLATION_POLL);
            }
        });
        let stop = MonitorStop {
            finished,
            monitor_thread: monitor.thread().clone(),
        };
        let result = operation(&cancellation);
        drop(stop);
        monitor.join().map_err(|_| LiveStandardFailure::Error)?;
        match reason.load(Ordering::Acquire) {
            MONITOR_DEVICE_LOST => Err(LiveStandardFailure::DeviceLost),
            MONITOR_IDLE | MONITOR_CANCELLED => Ok(result),
            _ => Err(LiveStandardFailure::Error),
        }
    })
}

struct MonitorStop {
    finished: Arc<AtomicBool>,
    monitor_thread: thread::Thread,
}

impl Drop for MonitorStop {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);
        self.monitor_thread.unpark();
    }
}

const fn standard_match(outcome: StandardMatchOutcome) -> MatchOutcome {
    match outcome {
        StandardMatchOutcome::Matched(_) => MatchOutcome::Matched,
        StandardMatchOutcome::NoMatch => MatchOutcome::NoMatch,
        StandardMatchOutcome::Cancelled => MatchOutcome::Cancelled,
        StandardMatchOutcome::TimedOut => MatchOutcome::TimedOut,
    }
}

fn match_message(outcome: StandardMatchOutcome) -> ServerMessage {
    terminal(match outcome {
        StandardMatchOutcome::Matched(identity) => TerminalOutcome::Matched(identity.id),
        StandardMatchOutcome::NoMatch => TerminalOutcome::NoMatch,
        StandardMatchOutcome::Cancelled => TerminalOutcome::Cancelled,
        StandardMatchOutcome::TimedOut => TerminalOutcome::Error,
    })
}

fn map_failure(result: Result<ServerMessage, LiveStandardFailure>) -> ServerMessage {
    result.unwrap_or_else(|failure| {
        terminal(match failure {
            LiveStandardFailure::Cancelled => TerminalOutcome::Cancelled,
            LiveStandardFailure::CapacityFull => TerminalOutcome::CapacityFull,
            LiveStandardFailure::DeviceLost => TerminalOutcome::DeviceLost,
            LiveStandardFailure::Duplicate => TerminalOutcome::Duplicate,
            LiveStandardFailure::Error => TerminalOutcome::Error,
        })
    })
}

fn terminal(outcome: TerminalOutcome) -> ServerMessage {
    ServerMessage::Terminal(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::time::Instant;

    use crate::auth_feedback::{AuthenticationFeedback, FeedbackAction};
    use crate::auth_protocol::{AccessPolicy, PeerAddressFamily, PeerMetadata};
    use crate::auth_session::{BrokerSessionCoordinator, StandardSessionDecision};
    use crate::identity_metadata::{IdentityMetadata, IdentityMetadataEntry};
    use crate::standard_fingerprint_protocol::{Identity, Username};
    use t1_bridge::transport::{FramePart, TransportError};

    const OWNER_UID: u32 = 42_000;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Call {
        List,
        Enroll(FingerLabel),
        Verify(IdentityId),
        Identify,
        Delete(IdentityId),
    }

    struct FakeRuntime {
        calls: Vec<Call>,
    }

    impl FakeRuntime {
        fn new() -> Self {
            Self { calls: Vec::new() }
        }
    }

    impl StandardOperationRuntime for FakeRuntime {
        fn list(&mut self, _active: &ActiveStandardOperation) -> ServerMessage {
            self.calls.push(Call::List);
            ServerMessage::IdentityList {
                owner: Some(username()),
                identities: vec![Identity {
                    id: id(2),
                    finger: FingerLabel::LeftIndex,
                }],
            }
        }

        fn enroll(
            &mut self,
            _active: &ActiveStandardOperation,
            _account: &ResolvedStandardAccount,
            finger: FingerLabel,
            progress: &mut dyn FnMut(EnrollProgress),
        ) -> ServerMessage {
            self.calls.push(Call::Enroll(finger));
            progress(EnrollProgress::new(1, 100).unwrap());
            terminal(TerminalOutcome::Enrolled(id(3)))
        }

        fn verify(
            &mut self,
            _active: &ActiveStandardOperation,
            _account: &ResolvedStandardAccount,
            identity: IdentityId,
        ) -> ServerMessage {
            self.calls.push(Call::Verify(identity));
            terminal(TerminalOutcome::Matched(identity))
        }

        fn identify(
            &mut self,
            _active: &ActiveStandardOperation,
            _account: &ResolvedStandardAccount,
        ) -> ServerMessage {
            self.calls.push(Call::Identify);
            terminal(TerminalOutcome::Matched(id(2)))
        }

        fn delete(
            &mut self,
            _active: &ActiveStandardOperation,
            _account: &ResolvedStandardAccount,
            identity: IdentityId,
        ) -> ServerMessage {
            self.calls.push(Call::Delete(identity));
            terminal(TerminalOutcome::Completed)
        }
    }

    fn username() -> Username {
        Username::new("synthetic-owner").unwrap()
    }

    fn account() -> ResolvedStandardAccount {
        let username = username();
        ResolvedStandardAccount::new(&username, &username, OWNER_UID).unwrap()
    }

    fn id(value: u8) -> IdentityId {
        IdentityId::new([value; 16]).unwrap()
    }

    fn active_for(
        operation: ResolvedStandardOperation,
        owner: Option<AccessPolicy>,
    ) -> (BrokerSessionCoordinator, ActiveStandardOperation) {
        let mut coordinator = BrokerSessionCoordinator::default();
        let peer = PeerMetadata {
            address_family: PeerAddressFamily::Local,
            user_id: 0,
            group_id: 0,
        };
        let StandardSessionDecision::Start(active) =
            coordinator.dispatch_standard(peer, owner, operation)
        else {
            panic!("synthetic standard operation must start")
        };
        (coordinator, active)
    }

    fn owner_policy() -> AccessPolicy {
        AccessPolicy::new(OWNER_UID).unwrap()
    }

    #[test]
    fn exact_operations_route_once_and_return_bound_completions() {
        let cases = [
            (
                ResolvedStandardOperation::ListIdentities,
                Call::List,
                ServerMessage::IdentityList {
                    owner: Some(username()),
                    identities: vec![Identity {
                        id: id(2),
                        finger: FingerLabel::LeftIndex,
                    }],
                },
                Some(owner_policy()),
            ),
            (
                ResolvedStandardOperation::Enroll {
                    account: account(),
                    finger: FingerLabel::RightThumb,
                },
                Call::Enroll(FingerLabel::RightThumb),
                terminal(TerminalOutcome::Enrolled(id(3))),
                None,
            ),
            (
                ResolvedStandardOperation::Verify {
                    account: account(),
                    identity: id(2),
                },
                Call::Verify(id(2)),
                terminal(TerminalOutcome::Matched(id(2))),
                Some(owner_policy()),
            ),
            (
                ResolvedStandardOperation::Identify { account: account() },
                Call::Identify,
                terminal(TerminalOutcome::Matched(id(2))),
                Some(owner_policy()),
            ),
            (
                ResolvedStandardOperation::DeleteIdentity {
                    account: account(),
                    identity: id(2),
                },
                Call::Delete(id(2)),
                terminal(TerminalOutcome::Completed),
                Some(owner_policy()),
            ),
        ];

        for (operation, expected_call, expected_result, owner) in cases {
            let (_coordinator, active) = active_for(operation, owner);
            let mut runtime = FakeRuntime::new();
            let mut progress = Vec::new();
            let completion = run_with_runtime(
                &active,
                &mut |event| progress.push(event.completed_stage()),
                &mut runtime,
            );

            assert_eq!(runtime.calls, [expected_call]);
            assert_eq!(completion.result(), &expected_result);
            if matches!(expected_call, Call::Enroll(_)) {
                assert_eq!(progress, [1]);
            } else {
                assert!(progress.is_empty());
            }
        }
    }

    #[test]
    fn delivered_cancellation_stops_before_runtime_dispatch() {
        let (mut coordinator, active) = active_for(
            ResolvedStandardOperation::Identify { account: account() },
            Some(owner_policy()),
        );
        assert!(coordinator.standard_client_disconnected(&active));
        let mut runtime = FakeRuntime::new();

        let completion = run_with_runtime(&active, &mut |_| {}, &mut runtime);

        assert!(runtime.calls.is_empty());
        assert_eq!(completion.result(), &terminal(TerminalOutcome::Cancelled));
    }

    #[test]
    fn cancellation_monitor_reaches_the_same_sep_signal() {
        let (mut coordinator, active) = active_for(
            ResolvedStandardOperation::Verify {
                account: account(),
                identity: id(2),
            },
            Some(owner_policy()),
        );
        let observed = with_standard_cancellation_monitor(
            &active,
            || true,
            |cancellation| {
                assert!(coordinator.standard_client_disconnected(&active));
                let deadline = Instant::now() + Duration::from_secs(1);
                while !cancellation.is_cancelled() && Instant::now() < deadline {
                    thread::yield_now();
                }
                cancellation.is_cancelled()
            },
        );

        assert_eq!(observed, Ok(true));
    }

    #[test]
    fn device_loss_cancels_sep_and_overrides_a_late_result() {
        let (_coordinator, active) = active_for(
            ResolvedStandardOperation::Verify {
                account: account(),
                identity: id(2),
            },
            Some(owner_policy()),
        );
        let present = AtomicBool::new(true);
        let observed = with_standard_cancellation_monitor(
            &active,
            || present.load(Ordering::Acquire),
            |cancellation| {
                present.store(false, Ordering::Release);
                let deadline = Instant::now() + Duration::from_secs(1);
                while !cancellation.is_cancelled() && Instant::now() < deadline {
                    thread::yield_now();
                }
                7
            },
        );

        assert_eq!(observed, Err(LiveStandardFailure::DeviceLost));
    }

    #[test]
    fn device_loss_token_cancels_the_bridge_operation_too() {
        let (_coordinator, active) = active_for(
            ResolvedStandardOperation::Verify {
                account: account(),
                identity: id(2),
            },
            Some(owner_policy()),
        );
        let cancellation = StandardCancellation::new();

        assert!(!standard_operation_cancelled(&active, &cancellation));
        cancellation.cancel();
        assert!(standard_operation_cancelled(&active, &cancellation));
        assert!(!close_standard_operation_cancellation(
            &active,
            &cancellation
        ));
    }

    #[test]
    fn typed_failure_mapping_never_carries_an_identity() {
        for (failure, expected) in [
            (LiveStandardFailure::Cancelled, TerminalOutcome::Cancelled),
            (
                LiveStandardFailure::CapacityFull,
                TerminalOutcome::CapacityFull,
            ),
            (LiveStandardFailure::DeviceLost, TerminalOutcome::DeviceLost),
            (LiveStandardFailure::Duplicate, TerminalOutcome::Duplicate),
            (LiveStandardFailure::Error, TerminalOutcome::Error),
        ] {
            assert_eq!(map_failure(Err(failure)), terminal(expected));
        }
        assert_eq!(
            match_message(StandardMatchOutcome::TimedOut),
            terminal(TerminalOutcome::Error)
        );
    }

    #[test]
    fn verify_feedback_uses_the_standard_visible_result() {
        struct Feedback(Vec<FeedbackAction>);

        impl AuthenticationFeedback for Feedback {
            type Error = Infallible;

            fn apply(&mut self, action: FeedbackAction) -> Result<(), Self::Error> {
                self.0.push(action);
                Ok(())
            }
        }

        let catalog = StandardIdentityCatalog::reconcile(
            Some(
                IdentityMetadata::new(
                    username(),
                    vec![
                        IdentityMetadataEntry {
                            id: id(1),
                            finger: None,
                        },
                        IdentityMetadataEntry {
                            id: id(2),
                            finger: Some(FingerLabel::LeftIndex),
                        },
                        IdentityMetadataEntry {
                            id: id(3),
                            finger: Some(FingerLabel::RightThumb),
                        },
                    ],
                )
                .unwrap(),
            ),
            username(),
            &[id(1), id(2), id(3)],
        )
        .unwrap();

        for raw_match in [id(1), id(3)] {
            let visible = evaluate_verify(
                &catalog,
                id(2),
                IdentityMatchOutcome::Matched(raw_match.as_bytes()),
            )
            .unwrap();
            let mut feedback = Feedback(Vec::new());
            assert_eq!(
                apply_authentication_feedback::<_, ()>(
                    Ok(standard_match(visible)),
                    Some(&mut feedback)
                ),
                Ok(MatchOutcome::NoMatch)
            );
            assert_eq!(
                feedback.0,
                [FeedbackAction::ShowRetry, FeedbackAction::PauseAfterRetry]
            );
        }
    }

    #[test]
    fn cold_restore_reopens_once_before_matching() {
        let mut calls = 0;
        let result = complete_match_after_restore(|| {
            calls += 1;
            Ok(if calls == 1 {
                LiveMatchPass::Restored
            } else {
                LiveMatchPass::Matched(IdentityMatchOutcome::Matched([0x55; 16]))
            })
        });

        assert_eq!(result, Ok(IdentityMatchOutcome::Matched([0x55; 16])));
        assert_eq!(calls, 2);
    }

    #[test]
    fn warm_match_uses_one_lease_and_repeated_restore_fails_closed() {
        let mut warm_calls = 0;
        assert_eq!(
            complete_match_after_restore(|| {
                warm_calls += 1;
                Ok(LiveMatchPass::Matched(IdentityMatchOutcome::NoMatch))
            }),
            Ok(IdentityMatchOutcome::NoMatch)
        );
        assert_eq!(warm_calls, 1);

        let mut restore_calls = 0;
        assert_eq!(
            complete_match_after_restore(|| {
                restore_calls += 1;
                Ok(LiveMatchPass::Restored)
            }),
            Err(LiveStandardFailure::Error)
        );
        assert_eq!(restore_calls, 2);
    }

    #[test]
    fn device_loss_is_distinct_from_local_and_protocol_setup_failure() {
        let stream_loss = LiveBiometricError::Command(BridgeCommandError::Session(
            SessionError::Transport(TransportError::UnexpectedEof {
                part: FramePart::Header,
            }),
        ));
        let local_failure = LiveBiometricError::Command(BridgeCommandError::RequestIdUnavailable);

        assert!(live_transport_is_device_loss(&stream_loss));
        assert!(!live_transport_is_device_loss(&local_failure));
        assert_eq!(
            map_live_connection_error(&LiveConnectionError::Socket),
            LiveStandardFailure::DeviceLost
        );
        assert_eq!(
            map_live_connection_error(&LiveConnectionError::ClientVersion),
            LiveStandardFailure::Error
        );
    }

    #[test]
    fn monitor_thread_is_joined_after_normal_completion() {
        let (_coordinator, active) = active_for(
            ResolvedStandardOperation::Identify { account: account() },
            Some(owner_policy()),
        );
        let calls = AtomicUsize::new(0);
        let result = with_standard_cancellation_monitor(
            &active,
            || true,
            |_| {
                calls.fetch_add(1, AtomicOrdering::Relaxed);
                7
            },
        );
        assert_eq!(result, Ok(7));
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 1);
    }
}
