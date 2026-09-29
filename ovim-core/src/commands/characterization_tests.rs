//! Characterization of every ex command through both entry points (OV-00487).
//!
//! Each row runs one command line on a fresh editor twice: through the API
//! entry point (`InputHandler::execute_command_api`, what `ovim exec` and the
//! GUI use) and through the interactive one (`InputHandler::execute_command_string`,
//! what the `:` prompt, keymaps and Lua use; its outcome is only visible on the
//! status line or hover popup). It pins the result kind and message, buffer
//! text, cursor and a few side effects.
//!
//! `interactive` is set only where the two entry points disagree; every such
//! row says why. Rows whose behaviour differs from vim say so with a `vim:`
//! note (verified with `nvim --clean --headless`, NVIM v0.12.2).

use crate::command_result::CommandResult;
use crate::editor::{Editor, InputHandler};
use crate::unicode::GraphemeCol;
use crate::{KeyCode, KeyEvent, Modifiers};

/// Default fixture: indented lines so first-non-blank cursor rules show.
const FIX: &str = "c1\nb2\n  a3\n  d4a";

#[derive(Clone, Copy, Debug)]
enum M {
    Is(&'static str),
    Starts(&'static str),
    Has(&'static str),
}

impl M {
    fn matches(self, text: &str) -> bool {
        match self {
            M::Is(expected) => text == expected,
            M::Starts(prefix) => text.starts_with(prefix),
            M::Has(part) => text.contains(part),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Out {
    Silent,
    Ok(M),
    Err(M),
}

type Check = fn(&Editor) -> Result<(), String>;

struct Case {
    cmd: &'static str,
    text: &'static str,
    cursor: (usize, usize),
    /// Normal-mode keys typed before the command (marks, edits).
    keys: &'static str,
    /// Ex commands run through the API before the command under test.
    pre: &'static [&'static str],
    /// Load `text` from a temp file named `f.txt` instead of an unnamed buffer.
    /// `{dir}` in `cmd` expands to that directory.
    file: bool,
    api: Out,
    /// Set only where the interactive entry point differs from the API.
    interactive: Option<Out>,
    text_after: Option<&'static str>,
    cursor_after: Option<(usize, usize)>,
    quits: bool,
    check: Option<Check>,
}

fn case(cmd: &'static str) -> Case {
    Case {
        cmd,
        text: FIX,
        cursor: (1, 1),
        keys: "",
        pre: &[],
        file: false,
        api: Out::Silent,
        interactive: None,
        text_after: None,
        cursor_after: None,
        quits: false,
        check: None,
    }
}

impl Case {
    fn text(mut self, text: &'static str) -> Self {
        self.text = text;
        self
    }
    fn at(mut self, line: usize, col: usize) -> Self {
        self.cursor = (line, col);
        self
    }
    fn keys(mut self, keys: &'static str) -> Self {
        self.keys = keys;
        self
    }
    fn pre(mut self, pre: &'static [&'static str]) -> Self {
        self.pre = pre;
        self
    }
    fn file(mut self) -> Self {
        self.file = true;
        self
    }
    fn ok(mut self, m: M) -> Self {
        self.api = Out::Ok(m);
        self
    }
    fn is(self, text: &'static str) -> Self {
        self.ok(M::Is(text))
    }
    fn err(mut self, m: M) -> Self {
        self.api = Out::Err(m);
        self
    }
    fn fails(self, text: &'static str) -> Self {
        self.err(M::Is(text))
    }
    fn unknown(self) -> Self {
        let message: &'static str =
            Box::leak(format!("E492: Not an editor command: {}", self.cmd).into_boxed_str());
        self.fails(message)
    }
    fn interactive(mut self, out: Out) -> Self {
        self.interactive = Some(out);
        self
    }
    fn after(mut self, text: &'static str) -> Self {
        self.text_after = Some(text);
        self
    }
    fn cursor(mut self, line: usize, col: usize) -> Self {
        self.cursor_after = Some((line, col));
        self
    }
    fn quits(mut self) -> Self {
        self.quits = true;
        self
    }
    fn check(mut self, check: Check) -> Self {
        self.check = Some(check);
        self
    }
}

fn unnamed_register(editor: &Editor) -> String {
    editor.registers().get(Some('"'))
}

fn register_is(editor: &Editor, expected: &str) -> Result<(), String> {
    let actual = unnamed_register(editor);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "unnamed register {actual:?}, expected {expected:?}"
        ))
    }
}

fn buffer_text(editor: &Editor) -> String {
    editor.buffer().rope().to_string()
}

fn cases() -> Vec<Case> {
    use M::{Has, Is, Starts};
    vec![
        // ---- undo / redo ----
        case("u").keys("x").after("c1\nb2\n  a3\n  d4a\n"),
        case("undo").keys("x").after("c1\nb2\n  a3\n  d4a\n"),
        // vim: :un[do] — ovim accepts only the listed spellings.
        case("un").keys("x").unknown().after("c1\nb\n  a3\n  d4a\n"),
        case("red").keys("xu").after("c1\nb\n  a3\n  d4a\n"),
        case("redo").keys("xu").after("c1\nb\n  a3\n  d4a\n"),
        // ---- quit ----
        case("q").is("Quitting").quits(),
        case("quit").is("Quitting").quits(),
        case("q!").is("Quitting (forced)").quits(),
        case("quit!").is("Quitting (forced)").quits(),
        // vim: :q[uit] — every prefix from "q" works.
        case("qu").unknown(),
        // vim: "E37: No write since last change (add ! to override)".
        case("q")
            .keys("x")
            .fails("No write since last change (add ! to override)"),
        case("q").pre(&["tabnew"]).is("Tab closed. Now on tab 1"),
        case("qa").is("Quitting all").quits(),
        case("qall").is("Quitting all").quits(),
        case("qa!").is("Quitting all (forced)").quits(),
        case("qall!").is("Quitting all (forced)").quits(),
        case("qa")
            .keys("x")
            .fails("No write since last change (add ! to override)"),
        // vim: :quita[ll] is :qall.
        case("quitall").unknown(),
        case("cq").is("Quitting with error code 1").quits(),
        case("cquit").is("Quitting with error code 1").quits(),
        case("cq 3").is("Quitting with error code 3").quits(),
        case("cq x").fails("Invalid exit code: x"),
        // vim: "E488: Trailing characters: foo".
        case("q foo").unknown(),
        // vim: :x[it] / :exi[t] write when modified, then quit; :wqa / :xa.
        case("x").unknown(),
        case("xit").unknown(),
        case("exit").unknown(),
        case("wqa").unknown(),
        case("xa").unknown(),
        // vim: :clo[se] on the last window is "E444: Cannot close last window".
        case("close").unknown(),
        // ---- write ----
        // vim: "E32: No file name".
        case("w").fails("No file name"),
        case("write").fails("No file name"),
        case("w!").fails("No file name"),
        case("write!").fails("No file name"),
        case("wq").fails("No file name"),
        case("wq!").fails("No file name"),
        case("wa"),
        case("wall"),
        case("writeall"),
        case("wa!"),
        case("w").file().ok(Has("written")),
        case("w!").file().ok(Has("written")),
        case("wq").file().is("Saved and quitting").quits(),
        case("w {dir}/other.txt")
            .file()
            .ok(Has("other.txt\" 5L, 17C written"))
            .check(|editor| {
                // vim: `:w other` writes a copy and keeps editing f.txt;
                // ovim renames the buffer (that is `:saveas` in vim).
                match editor.buffer().file_path() {
                    Some(path) if path.ends_with("other.txt") => Ok(()),
                    other => Err(format!("buffer path {other:?}")),
                }
            }),
        // vim: :sav[eas].
        case("saveas {dir}/other.txt")
            .file()
            .err(Starts("E492: Not an editor command: saveas ")),
        case("update"),
        case("up"),
        case("update").keys("x").fails("No file name"),
        // ---- edit ----
        case("e").fails("No file name"),
        case("edit").fails("No file name"),
        case("e!").fails("No file to reload"),
        case("edit!").fails("No file to reload"),
        case("e")
            .keys("x")
            .fails("No write since last change (add ! to override)"),
        case("e").file().ok(Has("f.txt\" 5L reloaded")),
        case("e!")
            .file()
            .keys("x")
            .ok(Has("reloaded"))
            .after("c1\nb2\n  a3\n  d4a\n"),
        case("e {dir}/f.txt").file().ok(Starts("Editing: ")),
        case("edit {dir}/f.txt").file().ok(Starts("Editing: ")),
        // vim: :e[dit].
        case("ed").unknown(),
        // ---- tabs ----
        case("tabnew").is("Created tab 2"),
        case("tabe").is("Created tab 2"),
        case("tabedit").is("Created tab 2"),
        case("tabnew {dir}/f.txt").file().ok(Has("in tab 2")),
        case("tabe {dir}/new.txt")
            .file()
            .ok(Has("Created new file")),
        case("tabn").is("Tab 1"),
        case("tabnext").is("Tab 1"),
        case("tabnext").pre(&["tabnew"]).is("Tab 1"),
        case("tabp").is("Tab 1"),
        case("tabprev").is("Tab 1"),
        case("tabprevious").is("Tab 1"),
        case("tabfir").is("Tab 1"),
        case("tabfirst").is("Tab 1"),
        case("tabl").is("Tab 1"),
        case("tablast").pre(&["tabnew", "tabfirst"]).is("Tab 2"),
        case("tabc").fails("Cannot close last tab"),
        case("tabclose").fails("Cannot close last tab"),
        case("tabclose")
            .pre(&["tabnew"])
            .is("Tab closed. Now on tab 1"),
        case("tabo").is("Already only one tab"),
        case("tabonly").is("Already only one tab"),
        case("tabonly").pre(&["tabnew"]).is("Closed 1 tabs"),
        case("tabs").ok(Starts("> 1 ")),
        // vim: :tabN[ext], :tabm[ove].
        case("tabN").unknown(),
        case("tabm").unknown(),
        // ---- windows ----
        case("sp").ok(Starts("Split horizontally")),
        case("split").ok(Starts("Split horizontally")),
        case("vsp").ok(Starts("Split vertically")),
        case("vsplit").ok(Starts("Split vertically")),
        // vim: :vs[plit]; `:sp file` splits and edits file.
        case("vs").unknown(),
        case("sp {dir}/f.txt")
            .file()
            .err(Starts("E492: Not an editor command: sp ")),
        case("only").is("Already only one window"),
        case("on").is("Already only one window"),
        // ---- buffers ----
        case("ls").ok(Starts("%")),
        case("buffers").ok(Starts("%")),
        case("files").ok(Starts("%")),
        case("bn").is("Buffer 1 of 1: [No Name]"),
        case("bnext").is("Buffer 1 of 1: [No Name]"),
        case("bp").is("Buffer 1 of 1: [No Name]"),
        case("bprev").is("Buffer 1 of 1: [No Name]"),
        case("bprevious").is("Buffer 1 of 1: [No Name]"),
        // vim: :bN[ext].
        case("bN").unknown(),
        // vim: :bd on the last buffer leaves an empty buffer instead of quitting.
        case("bd").is("Last buffer deleted, quitting").quits(),
        case("bdelete").is("Last buffer deleted, quitting").quits(),
        case("bd")
            .keys("x")
            .fails("No write since last change (add ! to override)"),
        case("bd!")
            .keys("x")
            .is("Last buffer deleted, quitting")
            .quits(),
        case("bdelete!").is("Last buffer deleted, quitting").quits(),
        case("b 1"),
        case("buffer 1"),
        // vim: "E86: Buffer 9 does not exist".
        case("b 9"),
        // vim: bare :b stays on the current buffer.
        case("b").unknown(),
        // ---- information ----
        case("noh").is("Search highlighting cleared"),
        case("nohlsearch").is("Search highlighting cleared"),
        // vim: :noh[lsearch].
        case("nohl").unknown(),
        // The interactive path stores the command in `":` before running it,
        // so the listing includes the `:reg` being executed; the API does not.
        case("reg")
            .is("No registers in use")
            .interactive(Out::Ok(Is("\":: reg"))),
        case("registers")
            .is("No registers in use")
            .interactive(Out::Ok(Is("\":: registers"))),
        // The `reg {arg}` spelling reaches a second, differently formatted
        // listing further down the old chain.
        case("reg a")
            .is("No registers set")
            .interactive(Out::Ok(Is("--- Registers ---\n\":   reg a"))),
        case("marks").is("No marks set"),
        case("marks")
            .keys("ma")
            .ok(Starts("mark  line   col  file\n 'a")),
        case("marks a").keys("ma").ok(Starts("--- Marks ---")),
        case("f").is("\"[No Name]\" line 2 of 4 --50%--"),
        case("file").is("\"[No Name]\" line 2 of 4 --50%--"),
        case("f")
            .keys("x")
            .is("\"[No Name]\" [Modified] line 2 of 4 --50%--"),
        case("pwd").ok(Starts("/")),
        case("help keybindings").ok(Starts("Keybinding compatibility guide")),
        case("help keys").ok(Starts("Keybinding compatibility guide")),
        // vim: :h[elp].
        case("help").unknown(),
        case("colo").ok(Starts("Current: ")),
        case("colorscheme").ok(Starts("Current: ")),
        case("colo nosuch").err(Has("Available schemes")),
        case("colorscheme nosuch").err(Has("Available schemes")),
        case("recover").fails("No swap file exists for this buffer"),
        case("rec").fails("No swap file exists for this buffer"),
        case("clearaedits").is("Agent edit markers cleared."),
        // ---- join ----
        case("j").after("c1\nb2 a3\n  d4a\n"),
        case("join").after("c1\nb2 a3\n  d4a\n"),
        // vim: `:2,3j` joins the range; `:j!` keeps whitespace.
        case("2,3j").unknown(),
        // ---- set ----
        case("set nu").check(|editor| {
            editor
                .options
                .number
                .then_some(())
                .ok_or_else(|| "number not set".to_string())
        }),
        case("se nu"),
        case("set number"),
        case("set nonu"),
        case("set nu?").is("  number"),
        case("set bogus").err(Has("bogus")),
        // vim: bare :se[t] lists changed options.
        case("set").unknown(),
        case("se").unknown(),
        // The API passes the whole line to :set, which rejects `|`; the
        // interactive path splits at the bar and runs both.
        case("set nu | set rnu")
            .err(Has("|"))
            .interactive(Out::Silent),
        // ---- mappings ----
        case("nnoremap Q x"),
        case("nmap Q x"),
        case("map").is("No mappings"),
        case("nmap").is("No mappings"),
        case("nnoremap Q").is("No mapping found"),
        // vim: :nn[oremap].
        case("nn Q x").unknown(),
        case("unmap Q").fails("E31: No such mapping"),
        case("unmap").fails("E474: Invalid argument"),
        case("mapclear"),
        case("nmapclear"),
        // ---- line addresses ----
        case("3").is("Line 3").cursor(2, 0),
        case("100").is("Line 100").cursor(3, 0),
        case("0").is("Line 0").cursor(0, 0),
        case("$").cursor(3, 0),
        // Parsed as the line number 1: vim goes to line 3.
        case("+1").is("Line 1").cursor(0, 0),
        case("'a").keys("ggma").cursor(0, 0),
        case("'a").fails("E20: Mark not set"),
        // vim: `.+1` is line 3.
        case(".+1").unknown().cursor(1, 1),
        // ---- :d / :y ----
        case("2,3d").after("c1\n  d4a\n").cursor(1, 0),
        case("d").after("c1\n  a3\n  d4a\n").cursor(1, 0),
        case("delete").after("c1\n  a3\n  d4a\n"),
        // vim: the cursor lands on the first non-blank (column 2).
        case("d").at(3, 0).after("c1\nb2\n  a3\n").cursor(2, 0),
        // vim: :d[elete].
        case("de").unknown(),
        case("%d").after(""),
        // vim: "E16: Invalid range"; ovim clamps to the last line.
        case("10d").after("c1\nb2\n  a3\n"),
        // vim prompts before swapping a backwards range; ovim swaps.
        case("4,2d").after("c1\n"),
        case("'a,'bd")
            .fails("E20: Mark not set")
            .after("c1\nb2\n  a3\n  d4a\n"),
        case("'a,'bd")
            .keys("ggmajmb")
            .after("  a3\n  d4a\n")
            .cursor(0, 0),
        case("2,3d").check(|editor| register_is(editor, "b2\n  a3\n")),
        // vim: `:d x` deletes into register x, `:3d 2` deletes two lines.
        case("d x").unknown(),
        case("3d 2").unknown(),
        case("2,3y")
            .after("c1\nb2\n  a3\n  d4a\n")
            .cursor(1, 1)
            .check(|editor| register_is(editor, "b2\n  a3\n")),
        case("y").check(|editor| register_is(editor, "b2\n")),
        case("yank").check(|editor| register_is(editor, "b2\n")),
        case(".,+2y").check(|editor| register_is(editor, "b2\n  a3\n  d4a\n")),
        // vim: `:y a` yanks into register a.
        case("y a").unknown(),
        // ---- :sort ----
        // vim: without a range :sort sorts the whole buffer.
        case("sort")
            .is("1 lines sorted")
            .after("c1\nb2\n  a3\n  d4a\n"),
        case("%sort")
            .is("4 lines sorted")
            .after("  a3\n  d4a\nb2\nc1\n"),
        // The bang is taken for a `:!` filter separator.
        case("%sort!").unknown(),
        case("%sort u")
            .text("b\na\nb")
            .is("2 lines sorted")
            .after("a\nb\n"),
        case("%sort n")
            .text("10\n9\n100")
            .is("3 lines sorted")
            .after("9\n10\n100\n"),
        // vim: :sor[t].
        case("sor").unknown(),
        // ---- :t / :copy / :m / :move ----
        case("1t3")
            .is("1 line copied")
            .after("c1\nb2\n  a3\nc1\n  d4a\n")
            .cursor(3, 0),
        // vim: the cursor ends on the LAST copied line (5, not 4).
        case("1,2t$")
            .is("2 lines copied")
            // vim: no blank line before the copy.
            .after("c1\nb2\n  a3\n  d4a\n\nc1\nb2\n")
            .cursor(4, 0),
        case("t0")
            .is("1 line copied")
            .after("b2\nc1\nb2\n  a3\n  d4a\n")
            .cursor(0, 0),
        case("copy 0").is("1 line copied"),
        case("t.")
            .is("1 line copied")
            .after("c1\nb2\nb2\n  a3\n  d4a\n"),
        // vim: :co[py].
        case("co 0").unknown(),
        case("t").fails("E488: Trailing characters"),
        case("1t'z").fails("E20: Mark not set"),
        case("1tx").unknown(),
        case("1m$")
            .is("1 line moved")
            // vim: no blank line before the moved line.
            .after("b2\n  a3\n  d4a\n\nc1\n")
            .cursor(3, 0),
        case("m0").is("1 line moved").after("b2\nc1\n  a3\n  d4a\n"),
        case("move 0")
            .is("1 line moved")
            .after("b2\nc1\n  a3\n  d4a\n"),
        // vim: the cursor ends on the LAST moved line (3, not 2).
        case("1,2m3")
            .is("2 lines moved")
            .after("  a3\nc1\nb2\n  d4a\n")
            .cursor(1, 0),
        case("2,3m2").fails("E134: Move lines into themselves"),
        // vim: :m[ove], so `:mo` is :move.
        case("mo 0").unknown(),
        // ---- :s ----
        case("s/b/X/").after("c1\nX2\n  a3\n  d4a\n").cursor(1, 1),
        case("%s/a/X/g").after("c1\nb2\n  X3\n  d4X\n"),
        case("2,3s/./Z/").after("c1\nZ2\nZ a3\n  d4a\n"),
        case("%s/zz/y/").fails("E486: Pattern not found: zz"),
        case("%s/zz/y/e"),
        // A bad pattern is reported on the status line without an E-number,
        // so the API reported it as success.
        case("s/(/x/").is("Invalid regex pattern: ("),
        case("s//x/").fails("E35: No previous regular expression"),
        // vim: `:s/b` replaces with nothing.
        case("s/b").fails("E146: Invalid substitute syntax"),
        // vim: :s[ubstitute] and any non-alphanumeric delimiter.
        case("substitute/b/X/").unknown(),
        case("s#b#X#").unknown(),
        case("%s/a\\/b/X/").text("a/b").after("X\n"),
        case("%s/b/X/|%s/c/Y/").after("Y1\nX2\n  a3\n  d4a\n"),
        case("%s/a/X/gc").ok(Starts("replace with X (2 matches)")),
        // ---- :g / :v ----
        case("g/a/d")
            .is("Deleted 2 line(s)")
            .after("c1\nb2\n")
            .cursor(1, 0),
        case("v/a/d").is("Deleted 2 line(s)").after("  a3\n  d4a\n"),
        case("g!/a/d")
            .is("Deleted 2 line(s)")
            .after("  a3\n  d4a\n"),
        case("2,3g/a/d")
            .is("Deleted 1 line(s)")
            .after("c1\nb2\n  d4a\n"),
        case("g/a/").is("3:   a3\n4:   d4a"),
        case("g/a/s/a/Y/")
            .is("Substituted on 2 line(s)")
            .after("c1\nb2\n  Y3\n  d4Y\n"),
        case("g/a/y")
            .is("Yanked 2 line(s)")
            .check(|editor| register_is(editor, "  a3\n  d4a\n")),
        case("g/b|c/d")
            .text("a\nb\nc")
            .is("Deleted 2 line(s)")
            .after("a\n"),
        case("g/zz/d").is("No matching lines found"),
        case("g/(/d").is("Invalid regex pattern: ("),
        // vim: :g accepts any ex command.
        case("g/a/normal Ax").is("Unsupported global command: normal Ax"),
        // vim: :g[lobal], :v[global].
        case("global/a/d").unknown(),
        // ---- shell ----
        // The API ran `:!cmd` inline and returned its output; the interactive
        // path queues it for the frontend.
        case("!echo hi")
            .is("hi")
            .interactive(Out::Silent)
            .check(|editor| match &editor.build.pending_shell_command {
                // Only the interactive run queues.
                Some(pending) if pending.command == "echo hi" => Ok(()),
                None => Ok(()),
                Some(other) => Err(format!("queued {:?}", other.command)),
            }),
        case("!").fails("No previous shell command"),
        case(".!tr a-z A-Z")
            .is("1 lines filtered")
            .after("c1\nB2\n  a3\n  d4a\n"),
        case("%!sort")
            .is("4 lines filtered")
            .after("  a3\nb2\nc1\n  d4a\n"),
        case("r !echo inserted")
            .is("1 line inserted")
            .after("c1\nb2\ninserted\n  a3\n  d4a\n")
            .cursor(2, 0),
        // vim: `:0r` inserts above line 1.
        case("0r !echo top")
            .is("1 line inserted")
            .after("c1\ntop\nb2\n  a3\n  d4a\n"),
        // `:w !cmd` is not characterized here: the standard dispatcher takes
        // it for `:w {file}` and writes a file named "!cmd" into the working
        // directory, so the interactive pipe-to-command handler is dead code.
        // vim: the range keeps its line breaks when piped.
        case("2,3w !true").is("1 line written"),
        case("terminal")
            .fails("Interactive terminal sessions require the TUI frontend")
            .interactive(Out::Silent),
        case("term ls")
            .fails("Interactive terminal sessions require the TUI frontend")
            .interactive(Out::Silent),
        case("shell")
            .fails("Interactive terminal sessions require the TUI frontend")
            .interactive(Out::Silent),
        case("terminally").unknown(),
        // vim: "E484: Can't open file nosuch"; reported as success.
        case("r /nonexistent/nosuch").ok(Starts("Error reading file")),
        case("r {dir}/f.txt")
            .file()
            .ok(Starts("Read 4 lines from"))
            .after("c1\nb2\nc1\nb2\n  a3\n  d4a\n  a3\n  d4a\n"),
        // ---- quickfix / project ----
        case("copen").is("Quickfix list is empty"),
        // vim: :cope[n].
        case("cope").unknown(),
        case("cclose").is("Quickfix list cleared"),
        case("ccl").is("Quickfix list cleared"),
        case("cn").fails("Quickfix list is empty"),
        case("cnext").fails("Quickfix list is empty"),
        case("cp").fails("Quickfix list is empty"),
        case("cprev").fails("Quickfix list is empty"),
        case("cprevious").fails("Quickfix list is empty"),
        case("cfir").fails("Quickfix list is empty"),
        case("cfirst").fails("Quickfix list is empty"),
        case("cla").fails("Quickfix list is empty"),
        case("clast").fails("Quickfix list is empty"),
        case("cdo s/a/b/").fails("E42: No Errors"),
        case("cfdo update").fails("E42: No Errors"),
        // vim: "E471: Argument required".
        case("cdo").unknown(),
        case("grep").fails("E471: Argument required"),
        case("gr").fails("E471: Argument required"),
        case("vimgrep").fails("E471: Argument required"),
        case("vim").fails("E471: Argument required"),
        case("Problems bogus").fails("Problems: use all, warnings or errors"),
        case("Diagnostics bogus").fails("Problems: use all, warnings or errors"),
        case("ReplaceUndo").err(Has("")),
        case("ReplaceApply").err(Has("")),
        // ---- LSP ----
        case("LspRestart").fails("LSP is not enabled"),
        case("LspRestart rust").fails("LSP is not enabled"),
        case("LspExec").fails("Usage: LspExec <command> [json arguments...]"),
        case("LspExec foo {bad").err(Starts("LspExec: arguments must be JSON values")),
        case("LspRename x").is("Renaming to 'x'..."),
        case("LspRename").unknown(),
        case("format").is("Formatting document..."),
        case("Format").is("Formatting document..."),
        case("LspInfo"),
        case("LspInstall"),
        case("LspManager"),
        // ---- run / test / debug ----
        case("RunJump x").fails("Usage: RunJump <line index>"),
        case("RunClear"),
        case("RunPrev"),
        case("RunNext"),
        case("TestPanel").is("Test panel opened"),
        case("TestToggle").is("Test panel opened"),
        case("TP").is("Test panel opened"),
        case("TestOutput").fails("No make/test output available"),
        case("MakeOutput").fails("No make/test output available"),
        case("debug").ok(Starts("Usage: :debug")),
        case("debug bogus").err(Starts("Unknown debug subcommand: 'bogus'")),
        case("eval 1+1").fails("Not stopped at a breakpoint"),
        case("eval").unknown(),
        case("DebugWatch x").is("Watching x"),
        case("DebugWatch").unknown(),
        case("DebugUnwatch").fails("No such watch (use :DebugUnwatch <number|expression>)"),
        case("DebugBreakpoints bogus").fails("Usage: :DebugBreakpoints [list|on|off|clear]"),
        case("DebugBreakpoints clear").is("All breakpoints removed"),
        case("DebugExpand x").fails("Not stopped at a breakpoint"),
        case("DebugPanel"),
        case("GitDiffLayout sideways").fails("GitDiffLayout: use split or unified"),
        // ---- sessions / workflows / AI / misc ----
        case("workflow bogus").err(Starts("Unknown workflow subcommand 'bogus'")),
        case("session bogus").err(Starts("Unknown session subcommand: 'bogus'")),
        case("session stop").fails("No active session to stop"),
        case("session start !!!").err(Has("Invalid session name")),
        case("browser").fails(
            "Could not open embedded browser: The embedded browser is unavailable in this frontend",
        ),
        // ---- :normal ----
        // vim: :norm[al] {keys} runs normal-mode keys, per line with a range.
        case("normal Ax").unknown(),
        case("norm Ax").unknown(),
        // ---- unknown / malformed ----
        case("foo").unknown(),
        case("Foo").unknown(),
        // vim: leading colons are skipped.
        case(":q").unknown(),
        // vim: an empty command line does nothing.
        case("").err(Starts("E492: Not an editor command:")),
    ]
}

fn run_keys(editor: &mut Editor, keys: &str) {
    for ch in keys.chars() {
        InputHandler::handle_key_event(editor, KeyEvent::new(KeyCode::Char(ch), Modifiers::NONE))
            .unwrap();
    }
}

struct Fixture {
    editor: Editor,
    _dir: Option<tempfile::TempDir>,
    cmd: String,
}

fn fixture(case: &Case) -> Fixture {
    let mut editor = Editor::with_content(case.text);
    let mut cmd = case.cmd.to_string();
    let dir = if case.file {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, format!("{}\n", case.text)).unwrap();
        editor.load_file(&path).unwrap();
        cmd = cmd.replace("{dir}", &dir.path().display().to_string());
        Some(dir)
    } else {
        None
    };
    editor
        .buffer_mut()
        .cursor_mut()
        .set_position(case.cursor.0, GraphemeCol(case.cursor.1));
    run_keys(&mut editor, case.keys);
    for pre in case.pre {
        let _ = InputHandler::execute_command_api(&mut editor, pre);
    }
    editor.set_status_message(String::new());
    editor.clear_hover();
    Fixture {
        editor,
        _dir: dir,
        cmd,
    }
}

fn describe(out: &CommandResult) -> String {
    match out {
        CommandResult::Success(success) => match &success.message {
            Some(message) => format!("Ok({message:?})"),
            None => "Silent".to_string(),
        },
        CommandResult::Error(error) => format!("Err({:?})", error.error),
    }
}

fn api_matches(expected: Out, actual: &CommandResult) -> bool {
    match (expected, actual) {
        (Out::Silent, CommandResult::Success(success)) => success.message.is_none(),
        (Out::Ok(m), CommandResult::Success(success)) => success
            .message
            .as_deref()
            .is_some_and(|text| m.matches(text)),
        (Out::Err(m), CommandResult::Error(error)) => m.matches(&error.error),
        _ => false,
    }
}

/// The interactive path only shows text: success and failure look alike.
fn shown_matches(expected: Out, editor: &Editor) -> bool {
    let status = editor.status_message();
    let hover = editor.hover_info().unwrap_or("");
    match expected {
        Out::Silent => status.is_empty() && hover.is_empty(),
        Out::Ok(m) | Out::Err(m) => m.matches(status) || m.matches(hover),
    }
}

fn check_state(case: &Case, editor: &Editor, failures: &mut Vec<String>, entry: &str) {
    let label = format!("{entry} {:?}", case.cmd);
    if let Some(expected) = case.text_after {
        let actual = buffer_text(editor);
        if actual != expected {
            failures.push(format!("{label}: buffer {actual:?}, expected {expected:?}"));
        }
    }
    if let Some((line, col)) = case.cursor_after {
        let cursor = editor.buffer().cursor();
        let actual = (cursor.line(), cursor.col().0);
        if actual != (line, col) {
            failures.push(format!(
                "{label}: cursor {actual:?}, expected {:?}",
                (line, col)
            ));
        }
    }
    if editor.should_quit() != case.quits {
        failures.push(format!(
            "{label}: should_quit {}, expected {}",
            editor.should_quit(),
            case.quits
        ));
    }
    if let Some(check) = case.check {
        if let Err(problem) = check(editor) {
            failures.push(format!("{label}: {problem}"));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_ex_command_through_both_entry_points() {
    let mut failures = Vec::new();
    let cases = cases();
    assert!(cases.len() >= 150, "only {} cases", cases.len());
    for case in &cases {
        // API entry point.
        let mut api = fixture(case);
        let result = InputHandler::execute_command_api(&mut api.editor, &api.cmd);
        if !api_matches(case.api, &result) {
            failures.push(format!(
                "api {:?}: got {}, expected {:?}",
                case.cmd,
                describe(&result),
                case.api
            ));
        }
        check_state(case, &api.editor, &mut failures, "api");

        // Interactive entry point.
        let mut interactive = fixture(case);
        InputHandler::execute_command_string(&mut interactive.editor, &interactive.cmd).unwrap();
        let expected = case.interactive.unwrap_or(case.api);
        if !shown_matches(expected, &interactive.editor) {
            failures.push(format!(
                "interactive {:?}: status {:?} hover {:?}, expected {:?}",
                case.cmd,
                interactive.editor.status_message(),
                interactive.editor.hover_info(),
                expected
            ));
        }
        check_state(case, &interactive.editor, &mut failures, "interactive");
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
