//! Local launch plans for Java / Kotlin tests, used when the language server
//! cannot say how to run a test (`hyperion.resolveLaunch` missing, still
//! indexing, or returning nothing).
//!
//! The plan is composed from the file alone: tree-sitter finds the
//! `@Test` / `@ParameterizedTest` methods and their (nested) classes, the
//! nearest `build.gradle(.kts)` / `pom.xml` decides the build tool and module,
//! and the build tool's own test filter selects what to run
//! (Gradle `--tests`, Maven `-Dtest=`). It has exactly the shape of a plan
//! from the server, so everything after it (console, JUnit results, debug
//! attach) is shared.

use std::path::{Path, PathBuf};

use super::nearest::{discover_tests, jvm_package, nearest_test, DiscoveredTest};
use super::runners::TestScope;
use crate::launch::plan::{gradle_program, maven_program, LaunchPlan, PlanKind, TaskPlan};
use crate::syntax::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildTool {
    Gradle,
    Maven,
}

struct Module {
    tool: BuildTool,
    /// Directory of the module that owns the file.
    dir: PathBuf,
    /// Workspace root (holds `settings.gradle(.kts)` / the top-level pom).
    root: PathBuf,
}

fn has_gradle_build(dir: &Path) -> bool {
    ["build.gradle", "build.gradle.kts"]
        .iter()
        .any(|f| dir.join(f).is_file())
}

fn has_gradle_settings(dir: &Path) -> bool {
    ["settings.gradle", "settings.gradle.kts"]
        .iter()
        .any(|f| dir.join(f).is_file())
}

fn find_module(file: &Path) -> Option<Module> {
    let mut dir = file.parent()?;
    loop {
        let gradle = has_gradle_build(dir);
        let maven = dir.join("pom.xml").is_file();
        if gradle || maven {
            let tool = if gradle {
                BuildTool::Gradle
            } else {
                BuildTool::Maven
            };
            let module = dir.to_path_buf();
            let mut root = module.clone();
            let mut up = dir.parent();
            while let Some(candidate) = up {
                let is_root = match tool {
                    BuildTool::Gradle => has_gradle_settings(candidate),
                    BuildTool::Maven => candidate.join("pom.xml").is_file(),
                };
                if is_root {
                    root = candidate.to_path_buf();
                } else if tool == BuildTool::Maven {
                    // Poms must be contiguous.
                    break;
                }
                up = candidate.parent();
            }
            if tool == BuildTool::Gradle && has_gradle_settings(&module) {
                root = module.clone();
            }
            return Some(Module {
                tool,
                dir: module,
                root,
            });
        }
        dir = dir.parent()?;
    }
}

/// Binary class name (`com.example.Outer$Inner`) of a discovered test's class.
fn binary_class(package: Option<&str>, classes: &[String]) -> String {
    let nested = classes.join("$");
    match package {
        Some(p) => format!("{p}.{nested}"),
        None => nested,
    }
}

/// Test selection for the build tool's filter.
struct Selection {
    /// `(class, method)`; a `None` method selects the whole class.
    filters: Vec<(String, Option<String>)>,
    label: String,
    class_name: Option<String>,
    method_name: Option<String>,
}

fn select(
    scope: TestScope,
    tests: &[DiscoveredTest],
    package: Option<&str>,
    cursor_line: usize,
    file: &Path,
) -> Result<Selection, String> {
    match scope {
        TestScope::Nearest => {
            let test = nearest_test(tests, cursor_line)
                .ok_or_else(|| "No test found near cursor".to_string())?;
            let class = binary_class(package, &test.namespaces);
            let simple = test.namespaces.last().cloned().unwrap_or_default();
            // Parameterized invocations are reported as `name(int)` etc., so
            // the method filter is a plain name either way; the flavor only
            // matters for how results are matched afterwards.
            Ok(Selection {
                filters: vec![(class.clone(), Some(test.name.clone()))],
                label: format!("{simple}.{}", test.name),
                class_name: Some(class),
                method_name: Some(test.name.clone()),
            })
        }
        TestScope::File => {
            let mut chains: Vec<Vec<String>> = Vec::new();
            for test in tests {
                if !chains.contains(&test.namespaces) {
                    chains.push(test.namespaces.clone());
                }
            }
            if chains.is_empty() {
                return Err("No tests found in this file".to_string());
            }
            let filters: Vec<_> = chains
                .iter()
                .map(|c| (binary_class(package, c), None))
                .collect();
            let name = file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok(Selection {
                class_name: (filters.len() == 1).then(|| filters[0].0.clone()),
                filters,
                label: name,
                method_name: None,
            })
        }
        TestScope::Suite => Ok(Selection {
            filters: Vec::new(),
            label: "all tests".to_string(),
            class_name: None,
            method_name: None,
        }),
    }
}

/// A locally composed plan plus where in the file to ask the server about it.
pub struct LocalTest {
    pub plan: LaunchPlan,
    /// LSP position (line, UTF-16 column) inside the selected test / class.
    pub anchor: (usize, usize),
    /// The cursor is already inside the selected test (use it as is).
    pub cursor_inside: bool,
}

/// Line and indentation of the first line matching `class|object <name>`.
fn class_anchor(source: &str, name: &str) -> Option<(usize, usize)> {
    source.lines().enumerate().find_map(|(i, line)| {
        let pos = line.find(name)?;
        let before = line[..pos].trim_end();
        let is_decl = before.ends_with("class") || before.ends_with("object");
        let after = line[pos + name.len()..].chars().next();
        let boundary = after.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        (is_decl && boundary).then(|| (i, line.len() - line.trim_start().len()))
    })
}

/// Composes a test plan for the file, or explains why it cannot.
pub fn local_test_plan(
    scope: TestScope,
    file: &Path,
    source: &str,
    cursor_line: usize,
    lang: Language,
) -> Result<LocalTest, String> {
    let tests = discover_tests(lang, source);
    let package = jvm_package(source);
    let module = find_module(file).ok_or_else(|| {
        "No Gradle or Maven project found for this file (looked for build.gradle(.kts) / pom.xml)"
            .to_string()
    })?;
    let selection = select(scope, &tests, package.as_deref(), cursor_line, file)?;
    let anchor = match scope {
        TestScope::Nearest => nearest_test(&tests, cursor_line).map(|t| {
            let indent = source
                .lines()
                .nth(t.line)
                .map(|l| l.len() - l.trim_start().len())
                .unwrap_or(0);
            (t.line, indent)
        }),
        _ => tests
            .first()
            .and_then(|t| t.namespaces.first())
            .and_then(|name| class_anchor(source, name)),
    }
    .unwrap_or((cursor_line, 0));
    let cursor_inside = nearest_test(&tests, cursor_line)
        .is_some_and(|t| t.line <= cursor_line && cursor_line <= t.end_line);
    let rel = module
        .dir
        .strip_prefix(&module.root)
        .unwrap_or(Path::new(""));

    let (argv, debug_argv, cwd, reports_dir) = match module.tool {
        BuildTool::Gradle => {
            let project_path: String = rel
                .components()
                .map(|c| format!(":{}", c.as_os_str().to_string_lossy()))
                .collect();
            // `cleanTest` first: an unchanged rerun would otherwise be
            // UP-TO-DATE and write no new reports.
            let mut argv = vec![
                gradle_program(&module.root),
                format!("{project_path}:cleanTest"),
                format!("{project_path}:test"),
            ];
            for (class, method) in &selection.filters {
                argv.push("--tests".to_string());
                argv.push(match method {
                    Some(m) => format!("{class}.{m}"),
                    None => class.clone(),
                });
            }
            argv.push("--console=plain".to_string());
            let mut debug = argv.clone();
            debug.push("--debug-jvm".to_string());
            (
                argv,
                debug,
                module.root.clone(),
                module.dir.join("build/test-results/test"),
            )
        }
        BuildTool::Maven => {
            let mut argv = vec![maven_program(&module.root)];
            if !selection.filters.is_empty() {
                let spec = selection
                    .filters
                    .iter()
                    .map(|(class, method)| match method {
                        Some(m) => format!("{class}#{m}"),
                        None => class.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                argv.push(format!("-Dtest={spec}"));
                argv.push("-Dsurefire.failIfNoSpecifiedTests=false".to_string());
                argv.push("-DfailIfNoTests=false".to_string());
            }
            if !rel.as_os_str().is_empty() {
                argv.push("-pl".to_string());
                argv.push(rel.to_string_lossy().into_owned());
                argv.push("-am".to_string());
            }
            argv.push("test".to_string());
            let mut debug = argv.clone();
            debug.insert(1, "-Dmaven.surefire.debug".to_string());
            (
                argv,
                debug,
                module.root.clone(),
                module.dir.join("target/surefire-reports"),
            )
        }
    };

    let plan = LaunchPlan {
        name: format!("{} (tests)", selection.label),
        kind: PlanKind::Test,
        language: Some(
            match lang {
                Language::Kotlin => "kotlin",
                _ => "java",
            }
            .to_string(),
        ),
        project_root: module.root.clone(),
        module_dir: Some(module.dir.clone()),
        build_tool: match module.tool {
            BuildTool::Gradle => "gradle",
            BuildTool::Maven => "maven",
        }
        .to_string(),
        build: None,
        launch: None,
        task: Some(TaskPlan {
            argv,
            debug_argv: Some(debug_argv),
            cwd,
            class_name: selection.class_name,
            method_name: selection.method_name,
            reports_dir: Some(reports_dir),
        }),
        attach: None,
        warnings: Vec::new(),
    };
    Ok(LocalTest {
        plan,
        anchor,
        cursor_inside,
    })
}
