use std::process::Command;

fn hotdata() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hotdata"))
}

#[test]
fn search_create_help_documents_vector_algorithm_flags() {
    let output = hotdata()
        .args(["search", "create", "--help"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--algorithm",
        "--nlist",
        "--probe-fraction",
        "--vector-precision",
    ] {
        assert!(help.contains(flag), "missing {flag} in help: {help}");
    }
    assert!(help.contains("possible values: hnsw, ivf"), "help: {help}");
    assert!(help.contains("default: hnsw"), "help: {help}");
    assert!(
        help.contains("possible values: float64, float32, float16, float8, int8"),
        "help: {help}"
    );
}
