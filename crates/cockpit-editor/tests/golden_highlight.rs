//! Golden tests for syntax highlighting token spans (spec §18.3 / M2.5).
//!
//! Each case highlights a Rust snippet and snapshots every emitted span as
//! `kind start..end "source text"`, so a grammar or mapping change shows up as
//! a reviewable diff.

use std::fmt::Write;

use cockpit_editor::Language;
use cockpit_editor::highlight::compute;

fn snapshot(case: &str, source: &str) -> String {
    snapshot_with(Language::Rust, case, source)
}

fn snapshot_with(language: Language, case: &str, source: &str) -> String {
    let spans = compute(language, source);
    let mut out = String::new();
    writeln!(out, "case:   {case}").unwrap();
    writeln!(out, "source: {source:?}").unwrap();
    writeln!(out, "spans:").unwrap();
    for span in spans {
        let text = &source[span.range.clone()];
        writeln!(
            out,
            "  {:<11} {:>3}..{:<3} {:?}",
            format!("{:?}", span.kind),
            span.range.start,
            span.range.end,
            text
        )
        .unwrap();
    }
    out
}

#[test]
fn golden_function() {
    insta::assert_snapshot!(snapshot("function", "fn main() {\n    return;\n}\n"));
}

#[test]
fn golden_struct_and_types() {
    insta::assert_snapshot!(snapshot(
        "struct_and_types",
        "struct Point {\n    x: i32,\n    y: i32,\n}\n"
    ));
}

#[test]
fn golden_comments_and_strings() {
    insta::assert_snapshot!(snapshot(
        "comments_and_strings",
        "// a greeting\nlet msg = \"hello\\n\";\n"
    ));
}

#[test]
fn golden_attribute_and_macro() {
    insta::assert_snapshot!(snapshot(
        "attribute_and_macro",
        "#[derive(Debug)]\nstruct S;\nfn f() { println!(\"hi\"); }\n"
    ));
}

#[test]
fn golden_go_basic() {
    let source = concat!(
        "// Package main greets the world.\n",
        "package main\n",
        "\n",
        "import \"fmt\"\n",
        "\n",
        "//go:generate stringer -type=Color\n",
        "type Color int\n",
        "\n",
        "type Greeter interface {\n",
        "\tHello() string\n",
        "}\n",
        "\n",
        "type World struct {\n",
        "\tName string\n",
        "}\n",
        "\n",
        "func (w *World) Hello() string {\n",
        "\treturn fmt.Sprintf(\"hello %s\", w.Name)\n",
        "}\n",
        "\n",
        "func pump(ch chan<- int) {\n",
        "\tch <- 1\n",
        "}\n",
        "\n",
        "func main() {\n",
        "\tw := &World{Name: \"world\"}\n",
        "\tfmt.Println(w.Hello())\n",
        "}\n",
    );
    insta::assert_snapshot!(snapshot_with(Language::Go, "go_basic", source));
}
