use clap::Parser;
use zerochain_daemon::cli::{Cli, Commands};

#[test]
fn parse_contribute_command() {
    let cli = Cli::try_parse_from([
        "zerochain",
        "--workspace",
        "/tmp/ws",
        "contribute",
        "--type",
        "insight",
        "--body",
        "found a pattern",
        "--parent",
        "c-abc",
        "--tag",
        "negative",
        "--metric",
        "name=bpb,value=1.9,direction=lower",
    ])
    .unwrap();
    match cli.command {
        Commands::Contribute {
            kind,
            body,
            parents,
            tags,
            metric,
        } => {
            assert_eq!(kind, "insight");
            assert_eq!(body, "found a pattern");
            assert_eq!(parents, vec!["c-abc".to_string()]);
            assert_eq!(tags, vec!["negative".to_string()]);
            assert!(metric.is_some());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_verify_command() {
    let cli = Cli::try_parse_from([
        "zerochain",
        "verify",
        "c-target00000000",
        "--verdict",
        "confirmed",
        "--body",
        "reproduced",
    ])
    .unwrap();
    match cli.command {
        Commands::Verify {
            target, verdict, ..
        } => {
            assert_eq!(target, "c-target00000000");
            assert_eq!(verdict, "confirmed");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_graph_command() {
    let cli = Cli::try_parse_from(["zerochain", "graph", "--view", "leaders", "--json"]).unwrap();
    match cli.command {
        Commands::Graph { view, json, .. } => {
            assert_eq!(view.as_deref(), Some("leaders"));
            assert!(json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_init_parents() {
    let cli = Cli::try_parse_from([
        "zerochain",
        "init",
        "--name",
        "w",
        "--parent",
        "c-a",
        "--parent",
        "c-b",
    ])
    .unwrap();
    match cli.command {
        Commands::Init { parents, .. } => {
            assert_eq!(parents, vec!["c-a".to_string(), "c-b".to_string()])
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
