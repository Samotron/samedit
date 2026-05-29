//! The `cockpit` cross-launch IPC service contract (M13.7).
//!
//! Lets the `cockpit-quick` launcher (and, by extension, the `cockpit-jot`
//! org built-ins) drive a *running* main cockpit: open a project, or dispatch
//! an arbitrary command. These are the two new variants the plan adds on top
//! of the surface v0.12 used to drive the jot pane.
//!
//! The messages are plain `serde` types — they ride as the payload of
//! `cockpit_ipc::Envelope` under [`ServiceId::Cockpit`](cockpit_ipc::ServiceId),
//! so this module has no dependency on the transport crate (mirrors
//! `cockpit-org::service`). `CommandId` is carried as a `String` on the wire
//! since it is not itself `Serialize`; the receiver rebuilds it.
//!
//! # Security
//!
//! `DispatchCommand` is intentionally unrestricted: the launcher socket is
//! user-only, which already implies trust. Anyone who can write to the socket
//! can drive the cockpit — that is by design, and documented here so it is a
//! deliberate choice rather than an oversight. The [`handle`] helper records
//! requests into a [`CockpitOutcome`] for the binary to apply; it performs no
//! action itself, keeping the contract pure and testable.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A request from the launcher (client) to a running cockpit (server).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CockpitRequest {
    /// Focus the cockpit window for `path` if it is open, else launch one.
    OpenProject { path: PathBuf },
    /// Run a registered command with string arguments. `command` is a
    /// `CommandId` rendered as its dotted string (e.g. `theme.switch`).
    DispatchCommand { command: String, args: Vec<String> },
}

/// A response from the cockpit service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CockpitResponse {
    /// The request was accepted.
    Ok,
    /// The request could not be served (unknown command, no such project).
    Error(String),
}

/// What a [`CockpitRequest`] asks the binary to do. The pure [`handle`]
/// translates a request into one of these without performing it, so the
/// effect application (window focus, command dispatch) stays in the binary
/// while the routing stays unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CockpitOutcome {
    /// Open or focus the project at this path.
    OpenProject(PathBuf),
    /// Dispatch this command id with these args.
    DispatchCommand { command: String, args: Vec<String> },
}

/// Translate a request into the effect the binary should apply, and the
/// response to send back. Pure — no side effects.
pub fn handle(request: CockpitRequest) -> (CockpitOutcome, CockpitResponse) {
    match request {
        CockpitRequest::OpenProject { path } => {
            (CockpitOutcome::OpenProject(path), CockpitResponse::Ok)
        }
        CockpitRequest::DispatchCommand { command, args } => (
            CockpitOutcome::DispatchCommand {
                command: command.clone(),
                args: args.clone(),
            },
            CockpitResponse::Ok,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_ipc::{Encoding, Envelope, ServiceId, read_message, write_message};
    use std::io::Cursor;

    #[test]
    fn handle_routes_open_project() {
        let (outcome, resp) = handle(CockpitRequest::OpenProject {
            path: "/code/app".into(),
        });
        assert_eq!(outcome, CockpitOutcome::OpenProject("/code/app".into()));
        assert_eq!(resp, CockpitResponse::Ok);
    }

    #[test]
    fn handle_routes_dispatch_command() {
        let (outcome, resp) = handle(CockpitRequest::DispatchCommand {
            command: "theme.switch".to_string(),
            args: vec!["mocha".to_string()],
        });
        assert_eq!(
            outcome,
            CockpitOutcome::DispatchCommand {
                command: "theme.switch".to_string(),
                args: vec!["mocha".to_string()],
            }
        );
        assert_eq!(resp, CockpitResponse::Ok);
    }

    #[test]
    fn requests_round_trip_through_the_real_ipc_codec() {
        let request = CockpitRequest::DispatchCommand {
            command: "org.capture".to_string(),
            args: vec!["t".to_string()],
        };
        let envelope = Envelope::new(ServiceId::Cockpit, request.clone());
        let mut buf = Vec::new();
        write_message(&mut buf, &envelope, Encoding::Cbor).expect("encode");
        let decoded: Envelope<CockpitRequest> =
            read_message(&mut Cursor::new(&buf)).expect("decode");
        assert_eq!(decoded.service, ServiceId::Cockpit);
        assert_eq!(decoded.payload, request);
    }
}
