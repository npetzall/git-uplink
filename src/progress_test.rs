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
fn printable_replaces_control_characters() {
    assert_eq!(printable("a\x1b[31mb\x07c\rd\ne"), "a?[31mb?c?d?e");
    assert_eq!(printable("keep\ttabs and text ✓"), "keep\ttabs and text ✓");
}

#[test]
fn format_step_line_plain_strips_escapes() {
    let outcome = StepOutcome::fail("bad\x1b]0;title\x07");
    let line = format_step_line(false, "Adopt \x1b[2Jsubject", &outcome);
    assert!(!line.contains('\x1b'));
    assert!(!line.contains('\x07'));
}

#[test]
fn spinner_stops_on_drop() {
    let spinner = Spinner::start("Working".into());
    std::thread::sleep(std::time::Duration::from_millis(20));
    let started = std::time::Instant::now();
    drop(spinner);
    assert!(started.elapsed() < std::time::Duration::from_millis(60));
}

#[test]
fn spinner_stops_promptly() {
    let spinner = Spinner::start("Working".into());
    std::thread::sleep(std::time::Duration::from_millis(20));
    let started = std::time::Instant::now();
    spinner.finish_and_clear();
    assert!(started.elapsed() < std::time::Duration::from_millis(60));
}
