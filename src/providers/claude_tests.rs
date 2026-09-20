use super::*;

#[test]
fn drop_archived_removes_matching_ids() {
    let mut sessions = BTreeMap::new();
    sessions.insert("keep".into(), AgentSession::default());
    sessions.insert("gone".into(), AgentSession::default());
    let archived = HashSet::from(["gone".to_string()]);
    assert!(drop_archived(&mut sessions, &archived));
    assert!(sessions.contains_key("keep"));
    assert!(!sessions.contains_key("gone"));
}
