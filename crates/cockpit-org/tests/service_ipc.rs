//! The Org IPC service contract (M12.6/M12.7) end-to-end: a cockpit "client"
//! request and the jot "server" response both ride over the real
//! `cockpit-ipc` codec, and the server applies the request to a live
//! `OrgRoot`.

use cockpit_ipc::{Encoding, Envelope, ServiceId, read_message, write_message};
use cockpit_org::OrgDate;
use cockpit_org::{OrgRequest, OrgResponse, OrgRoot, handle_request};

const TODAY: OrgDate = OrgDate {
    year: 2026,
    month: 5,
    day: 29,
};

/// Encode an envelope to bytes and decode it back, as the socket would.
fn over_the_wire<T>(env: &Envelope<T>) -> Envelope<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let mut buf = Vec::new();
    write_message(&mut buf, env, Encoding::Cbor).unwrap();
    read_message(&mut buf.as_slice()).unwrap()
}

#[test]
fn complete_request_round_trips_and_mutates_root() {
    let mut root = OrgRoot::from_files(
        "/org",
        [(
            "/org/tasks.org",
            "* TODO water plants\nSCHEDULED: <2026-05-29 Fri +1w>\n",
        )],
    );

    // Client side: build a request and ship it as an Org-service envelope.
    let request = OrgRequest::Complete {
        path: "/org/tasks.org".into(),
        line: 0,
        today: TODAY,
    };
    let req_env = over_the_wire(&Envelope::new(ServiceId::Jot, request));
    assert_eq!(req_env.service, ServiceId::Jot);

    // Server side: apply to the live root, ship the response back.
    let response = handle_request(&mut root, req_env.payload);
    let resp_env: Envelope<OrgResponse> = over_the_wire(&Envelope::new(ServiceId::Jot, response));

    match resp_env.payload {
        OrgResponse::Updated { source, .. } => {
            assert!(source.contains("SCHEDULED: <2026-06-05 Fri +1w>"));
        }
        other => panic!("expected Updated, got {other:?}"),
    }
    // The jot process's in-memory root reflects the bump.
    assert!(
        root.file("/org/tasks.org")
            .unwrap()
            .source
            .contains("<2026-06-05 Fri +1w>")
    );
}

#[test]
fn today_agenda_round_trips() {
    let mut root = OrgRoot::from_files(
        "/org",
        [("/org/a.org", "* TODO due\nSCHEDULED: <2026-05-29 Fri>\n")],
    );

    let req: Envelope<OrgRequest> = over_the_wire(&Envelope::new(
        ServiceId::Jot,
        OrgRequest::Today { today: TODAY },
    ));
    let resp = handle_request(&mut root, req.payload);
    let resp: Envelope<OrgResponse> = over_the_wire(&Envelope::new(ServiceId::Jot, resp));

    match resp.payload {
        OrgResponse::Agenda(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].title, "due");
            assert_eq!(items[0].date, Some((2026, 5, 29)));
        }
        other => panic!("expected Agenda, got {other:?}"),
    }
}
