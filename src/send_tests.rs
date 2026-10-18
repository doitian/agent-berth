use super::*;

#[test]
fn header_names_the_sending_agent() {
    let sender = Identity {
        provider: "claude".into(),
        session_id: "83844ac0-e963-46b0-9c67-c1b43aacd29c".into(),
    };
    assert_eq!(
        with_header("ship it\nnow", Some(&sender)),
        format!(
            "[agent-berth message from {} (claude)]\n\nship it\nnow",
            sender.id()
        )
    );
}

#[test]
fn no_identity_means_no_header() {
    assert_eq!(with_header("ship it", None), "ship it");
}

#[test]
fn arguments_are_joined_and_trailing_newlines_dropped() {
    assert_eq!(
        read_message(&["hello".into(), "world\n\n".into()]).unwrap(),
        "hello world"
    );
    assert!(read_message(&["  ".into()]).is_err());
}
