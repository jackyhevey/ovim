//! The ex command table: every command ovim knows, with vim's abbreviation
//! rules, bang and range policies, argument kind, buffer-kind contexts and
//! handler. Name resolution, tab completion and file-argument completion all
//! read this one table.

use super::contexts::{Contexts, Lifecycle};
use super::{edit, files, pattern, quickfix, shell, Ex};
use crate::command_result::CommandResult;
use crate::editor::Editor;

pub(crate) type Handler = fn(&mut Editor, &Ex) -> CommandResult;

/// How a command treats its argument text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgKind {
    /// No argument: anything after the name is "E488: Trailing characters".
    None,
    /// Free text; `|` starts the next command.
    Text,
    /// A file name (completed as a path); `|` starts the next command.
    /// `!cmd` instead takes the rest of the line (`:r !`, `:w !`).
    File,
    /// `/pat/rep/` — a `|` in the pattern is literal, after it separates.
    Substitute,
    /// The whole rest of the line, `|` included (`:g`, `:normal`, `:!`).
    Rest,
}

/// Which lines a command applies to without a typed range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RangePolicy {
    /// A range is "E481: No range allowed".
    None,
    /// Default: the cursor line.
    Line,
    /// Default: the cursor line; line 0 is allowed (`:0r`).
    LineOrZero,
    /// Default: the whole buffer (`:g`, `:sort`, `:w !`).
    Whole,
    /// A bare range: jump there, clamped to the buffer.
    Goto,
}

pub(crate) struct ExCommand {
    /// Names in vim's `:help` notation: `q[uit]` accepts `q`, `qu`, `qui` and
    /// `quit`. The first name is the canonical one.
    pub names: &'static [&'static str],
    pub bang: bool,
    pub range: RangePolicy,
    pub args: ArgKind,
    pub contexts: Contexts,
    /// Meaning in special buffers (chat scratch, commit message, pseudocode).
    pub lifecycle: Option<Lifecycle>,
    pub handler: Handler,
}

const fn ex(names: &'static [&'static str], handler: Handler) -> ExCommand {
    ExCommand {
        names,
        bang: false,
        range: RangePolicy::None,
        args: ArgKind::None,
        contexts: Contexts::EDITABLE,
        lifecycle: None,
        handler,
    }
}

impl std::fmt::Debug for ExCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ExCommand({})", self.names.join(", "))
    }
}

impl ExCommand {
    const fn bang(mut self) -> Self {
        self.bang = true;
        self
    }
    const fn range(mut self, range: RangePolicy) -> Self {
        self.range = range;
        self
    }
    const fn args(mut self, args: ArgKind) -> Self {
        self.args = args;
        self
    }
    /// Also allowed in the pseudocode reading view.
    const fn anywhere(mut self) -> Self {
        self.contexts = Contexts::ANY;
        self
    }
    const fn lifecycle(mut self, lifecycle: Lifecycle) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    /// The canonical full name (`quit` for `q[uit]`).
    #[cfg(test)]
    pub fn name(&self) -> String {
        full_name(self.names[0])
    }
}

use ArgKind as A;
use RangePolicy as R;

pub(crate) static COMMANDS: &[ExCommand] = &[
    // ---- line ranges and text ----
    ex(&[""], edit::goto_line).range(R::Goto).anywhere(),
    ex(&["d[elete]"], edit::delete).range(R::Line).args(A::Text),
    ex(&["y[ank]"], edit::yank).range(R::Line).args(A::Text),
    ex(&["j[oin]"], edit::join)
        .bang()
        .range(R::Line)
        .args(A::Text),
    ex(&["sor[t]"], edit::sort)
        .bang()
        .range(R::Whole)
        .args(A::Text),
    ex(&["t", "co[py]"], edit::copy)
        .range(R::Line)
        .args(A::Text),
    ex(&["m[ove]"], edit::move_lines)
        .range(R::Line)
        .args(A::Text),
    ex(&["p[rint]"], edit::print).range(R::Line),
    ex(&["u[ndo]"], edit::undo),
    ex(&["red[o]"], edit::redo),
    ex(&["norm[al]"], edit::normal)
        .bang()
        .range(R::Line)
        .args(A::Rest),
    // ---- patterns ----
    ex(&["s[ubstitute]"], pattern::substitute)
        .range(R::Line)
        .args(A::Substitute),
    ex(&["g[lobal]"], pattern::global)
        .bang()
        .range(R::Whole)
        .args(A::Rest),
    ex(&["v[global]"], pattern::global)
        .range(R::Whole)
        .args(A::Rest),
    // ---- shell ----
    ex(&["!"], shell::bang).range(R::Line).args(A::Rest),
    ex(&["r[ead]"], shell::read)
        .range(R::LineOrZero)
        .args(A::File),
    ex(&["ter[minal]", "sh[ell]"], shell::terminal).args(A::Rest),
    ex(&["w[rite]"], files::write)
        .bang()
        .range(R::Whole)
        .args(A::File)
        .lifecycle(Lifecycle::Write),
    // ---- quickfix ----
    ex(&["cdo"], quickfix::quickfix_do).args(A::Rest),
    ex(&["cfdo"], quickfix::quickfix_do).args(A::Rest),
];

/// The full form of a `:help`-style name: `quit` for `q[uit]`.
#[cfg(test)]
pub fn full_name(spec: &str) -> String {
    spec.replace(['[', ']'], "")
}

/// Whether `typed` is an accepted spelling of `spec`.
fn name_matches(spec: &str, typed: &str) -> bool {
    match spec.split_once('[') {
        Some((required, optional)) => {
            let optional = optional.trim_end_matches(']');
            typed.len() >= required.len()
                && typed.len() <= required.len() + optional.len()
                && typed.starts_with(required)
                && optional.starts_with(&typed[required.len()..])
        }
        None => spec == typed,
    }
}

/// Resolve a typed command name.
pub fn lookup(typed: &str) -> Option<&'static ExCommand> {
    COMMANDS
        .iter()
        .find(|command| command.names.iter().any(|spec| name_matches(spec, typed)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_notation_accepts_every_prefix_from_the_required_part() {
        assert!(name_matches("s[ubstitute]", "s"));
        assert!(name_matches("s[ubstitute]", "subst"));
        assert!(!name_matches("s[ubstitute]", "substitutes"));
        assert!(!name_matches("sor[t]", "so"));
        assert!(name_matches("t", "t"));
        assert!(!name_matches("t", "ta"));
    }

    #[test]
    fn every_name_is_unambiguous() {
        // Two entries accepting the same spelling would make the table order
        // decide silently.
        for command in COMMANDS {
            for spec in command.names {
                let full = full_name(spec);
                let required = spec.split_once('[').map_or(spec.len(), |(r, _)| r.len());
                for len in required..=full.len() {
                    let typed = &full[..len];
                    let owners: Vec<String> = COMMANDS
                        .iter()
                        .filter(|other| other.names.iter().any(|s| name_matches(s, typed)))
                        .map(ExCommand::name)
                        .collect();
                    assert_eq!(owners.len(), 1, "{typed:?} resolves to {owners:?}");
                }
            }
        }
    }
}
