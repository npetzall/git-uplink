use super::*;

#[test]
fn format_step_line_tty_icons() {
    let pass = StepOutcome::pass("done");
    assert!(format_step_line(true, "Fetch upstream", &pass).contains("✓"));
    let fail = StepOutcome::fail("no token");
    assert!(format_step_line(true, "Fetch upstream", &fail).contains("✗"));
}

#[test]
fn format_step_line_plain_tags() {
    let pass = StepOutcome::pass("done");
    let line = format_step_line(false, "Fetch upstream", &pass);
    assert!(line.starts_with("[ok  ]"));
}

#[test]
fn spinner_stops_promptly() {
    let spinner = Spinner::start("Working".into());
    std::thread::sleep(std::time::Duration::from_millis(20));
    let started = std::time::Instant::now();
    spinner.finish_and_clear();
    assert!(started.elapsed() < std::time::Duration::from_millis(60));
}
