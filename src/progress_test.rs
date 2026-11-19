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
