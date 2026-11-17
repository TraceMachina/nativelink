// Copyright 2026 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
