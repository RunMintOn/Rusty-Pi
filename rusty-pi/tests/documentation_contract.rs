use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("rusty-pi must be below the repository root")
        .to_path_buf()
}

fn read_repo_file(relative: &str) -> String {
    let path = repository_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

#[test]
fn readme_documents_current_entry_points_and_authoritative_docs() {
    let readme = read_repo_file("README.md");
    for required in [
        "--tui",
        "--resume",
        "--list-sessions",
        "--context",
        "docs/capabilities.md",
        "docs/architecture.md",
    ] {
        assert!(readme.contains(required), "README is missing {required}");
    }

    for relative in [
        "docs/capabilities.md",
        "docs/architecture.md",
        "docs/roadmap.md",
        "docs/adr",
        "rusty-pi/docs/tui.md",
        "rusty-pi/docs/known-issues.md",
    ] {
        assert!(
            repository_root().join(relative).exists(),
            "documented path does not exist: {relative}"
        );
    }
}

#[test]
fn execution_evidence_is_not_tracked_as_product_documentation() {
    assert!(
        !repository_root().join("EXECUTION-EVIDENCE.md").exists(),
        "execution evidence must remain outside the product repository"
    );
}

#[test]
fn authoritative_documents_do_not_reintroduce_retired_positioning() {
    let files = [
        "README.md",
        "SPEC.md",
        "AGENTS.md",
        "MAINTENANCE.md",
        "docs/capabilities.md",
        "docs/architecture.md",
        "docs/roadmap.md",
    ];
    let banned = [
        "当前 200 个测试",
        "200 tests",
        "TUI（占位）",
        "TUI is a placeholder",
        "当前只实现内存 session",
        "完整复刻 PI",
        "不改变用户功能体验",
        "所有设计全权参考原版",
        "全权参考原版",
    ];

    for file in files {
        let contents = read_repo_file(file);
        for phrase in banned {
            assert!(!contents.contains(phrase), "{file} contains retired phrase: {phrase}");
        }
    }
}

#[test]
fn source_positioning_does_not_claim_a_complete_rewrite() {
    let banned = ["Rust rewrite of", "rewrite of earendil-works/pi"];

    for file in ["rusty-pi/src/main.rs", "rusty-pi/src/lib.rs"] {
        let source = read_repo_file(file);
        for phrase in banned {
            assert!(
                !source.contains(phrase),
                "{file} contains retired positioning: {phrase}"
            );
        }
    }
}

#[test]
fn cli_help_uses_independent_product_positioning() {
    let output = Command::new(env!("CARGO_BIN_EXE_rusty-pi"))
        .arg("--help")
        .output()
        .expect("rusty-pi --help must run");
    assert!(output.status.success(), "--help failed: {output:?}");

    let mut help = String::from_utf8_lossy(&output.stdout).into_owned();
    help.push_str(&String::from_utf8_lossy(&output.stderr));
    let help = help.to_ascii_lowercase();
    assert!(
        help.contains("independent"),
        "help must identify an independent product: {help}"
    );
    assert!(!help.contains("rewrite"), "help must not claim a rewrite: {help}");
    assert!(
        !help.contains("compatibility"),
        "help must not imply compatibility: {help}"
    );
}

#[test]
fn adr_numbers_and_required_sections_are_unique() {
    let adr_dir = repository_root().join("docs/adr");
    let mut numbers = HashSet::new();
    let entries = fs::read_dir(&adr_dir).expect("ADR directory must be readable");

    for entry in entries {
        let path = entry.expect("ADR directory entry must be readable").path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("ADR filename must be UTF-8");
        assert!(name.len() >= 3, "ADR filename has no three-digit number: {name}");
        let prefix = &name[..3];
        assert!(
            prefix.chars().all(|character| character.is_ascii_digit()),
            "ADR prefix is not numeric: {name}"
        );
        assert!(numbers.insert(prefix.to_owned()), "duplicate ADR number: {prefix}");

        let contents = fs::read_to_string(&path).expect("ADR must be readable");
        let fields = ["Status:", "Context:", "Decision:", "Consequences:"];
        let lines: Vec<&str> = contents.lines().collect();
        for field in fields {
            let index = lines
                .iter()
                .position(|line| line.starts_with(field))
                .unwrap_or_else(|| panic!("{} is missing {field}", path.display()));
            let inline = lines[index][field.len()..].trim();
            let following = lines[index + 1..]
                .iter()
                .take_while(|line| !fields.iter().any(|marker| line.starts_with(marker)))
                .any(|line| !line.trim().is_empty());
            assert!(
                !inline.is_empty() || following,
                "{} has an empty {field}",
                path.display()
            );
        }
    }
}

#[test]
fn capability_matrix_defines_only_the_five_allowed_statuses() {
    let capabilities = read_repo_file("docs/capabilities.md");
    let start = capabilities
        .find("## Status definitions")
        .expect("capability status definition section is required");
    let end = capabilities[start..]
        .find("\n## Runtime")
        .map(|offset| start + offset)
        .expect("capability runtime section must follow status definitions");
    let definitions = &capabilities[start..end];
    let allowed = ["Available", "Partial", "Infrastructure", "Planned", "Not planned"];

    let defined: HashSet<&str> = definitions
        .lines()
        .filter_map(|line| {
            let first = line.split('|').nth(1)?.trim();
            allowed.iter().copied().find(|status| *status == first)
        })
        .collect();
    let expected: HashSet<&str> = allowed.into_iter().collect();
    assert_eq!(defined, expected, "status definition table changed");
}

#[test]
fn resource_reload_status_separates_infrastructure_from_orchestration() {
    let capabilities = read_repo_file("docs/capabilities.md");
    assert!(
        capabilities
            .lines()
            .any(|line| line == "| Resource reload infrastructure | Infrastructure |"),
        "resource reload infrastructure row is required"
    );
    assert!(
        capabilities
            .lines()
            .any(|line| line == "| Controller-owned resource reload orchestration | Planned |"),
        "controller-owned resource reload orchestration row is required"
    );
    assert!(!capabilities.contains("| Resource reload | Available |"));
}

#[test]
fn thinking_capabilities_separate_scaffolding_transport_and_configuration() {
    let capabilities = read_repo_file("docs/capabilities.md");
    for row in [
        "| Thinking types, metadata, events, and frontend scaffolding | Infrastructure |",
        "| Provider thinking/reasoning transport | Planned |",
        "| User-configurable thinking/reasoning | Planned |",
    ] {
        assert!(
            capabilities.lines().any(|line| line == row),
            "missing thinking row: {row}"
        );
    }
    assert!(!capabilities.contains("| Thinking/reasoning transport | Infrastructure |"));
}

#[test]
fn accepted_adrs_are_not_marked_superseded() {
    let adr_dir = repository_root().join("docs/adr");
    for entry in fs::read_dir(&adr_dir).expect("ADR directory must be readable") {
        let path = entry.expect("ADR directory entry must be readable").path();
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&path).expect("ADR must be readable");
        if contents.lines().any(|line| line == "Status: Accepted") {
            let superseded_by = contents
                .lines()
                .find_map(|line| line.strip_prefix("Superseded by:").map(str::trim))
                .unwrap_or_else(|| panic!("{} is missing Superseded by", path.display()));
            assert_eq!(superseded_by, "None", "accepted ADR is superseded: {}", path.display());
        }
    }

    let adr005 = read_repo_file("docs/adr/005-prompt-session-transition.md");
    assert!(adr005.contains("ADR 006 records the related future controller direction."));
    assert!(adr005.contains("It complements this decision rather than superseding it."));
}

#[test]
fn every_capability_table_data_row_uses_an_allowed_status() {
    let capabilities = read_repo_file("docs/capabilities.md");
    let allowed = ["Available", "Partial", "Infrastructure", "Planned", "Not planned"];
    let mut in_capability_table = false;
    let mut data_rows = 0;

    for line in capabilities.lines() {
        if line.starts_with("## ") {
            in_capability_table = false;
        }
        if line.trim() == "| Capability | Status |" {
            in_capability_table = true;
            continue;
        }
        if !in_capability_table {
            continue;
        }
        if line.trim().is_empty() || !line.trim_start().starts_with('|') {
            in_capability_table = false;
            continue;
        }

        let columns: Vec<&str> = line.split('|').map(str::trim).collect();
        if columns.len() < 3 || columns[1].chars().all(|character| character == '-') {
            continue;
        }
        let status = columns[2];
        assert!(
            allowed.contains(&status),
            "invalid capability status {status:?} in row: {line}"
        );
        data_rows += 1;
    }

    assert!(data_rows > 0, "capability table data rows were not scanned");
}

#[test]
fn historical_plans_are_marked_and_point_to_current_facts() {
    let files = [
        "tickets.md",
        "tickets/feature-audit.md",
        "tickets/spec-bare-terminal-architecture.md",
        "tickets/bare-terminal-capabilities.md",
        "tickets/crate-reference-bare-terminal.md",
        "tickets/prompt-next-agent.md",
    ];

    for file in files {
        let contents = read_repo_file(file);
        assert!(contents.contains("Historical"), "{file} is not marked Historical");
        assert!(
            contents.contains("docs/capabilities.md"),
            "{file} does not point to the current capability source"
        );
    }
}
