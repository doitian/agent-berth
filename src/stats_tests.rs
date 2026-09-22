use super::*;

#[test]
fn total_row_sums_each_status() {
    let stats = vec![
        ProviderStats {
            provider: "claude".into(),
            running: 2,
            waiting: 1,
            idle: 3,
            done: 0,
            total: 6,
        },
        ProviderStats {
            provider: "pi".into(),
            running: 0,
            waiting: 0,
            idle: 1,
            done: 4,
            total: 5,
        },
    ];
    let total = total_row(&stats);
    assert_eq!(total.running, 2);
    assert_eq!(total.waiting, 1);
    assert_eq!(total.idle, 4);
    assert_eq!(total.done, 4);
    assert_eq!(total.total, 11);
}
