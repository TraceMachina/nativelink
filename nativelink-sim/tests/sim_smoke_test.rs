use nativelink_error::Error;
use nativelink_sim::{Action, Backend, Event, Outcome, Scenario, Step, WorkerSpec};

#[test]
fn one_action_one_worker() -> Result<(), Error> {
    let reports = Scenario::new("one-action", 1)
        .workers(1, WorkerSpec::default())
        .at(100, Step::execute("c1", Action::new(1).run_ms(2_000)))
        .run()?;
    for report in &reports {
        assert_eq!(
            report.outcomes().get("c1"),
            Some(&Some(Outcome::Exited(0))),
            "{:?}\n{}",
            report.backend,
            report.trace()
        );
        assert_eq!(report.count(|e| matches!(e, Event::ExecStart { .. })), 1);
    }
    assert_eq!(reports.len(), 2);
    assert!(reports.iter().any(|r| r.backend == Backend::FakeRedis));
    Ok(())
}
