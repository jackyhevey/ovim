# Running and Debugging

ovim runs and debugs JVM programs (Java, Kotlin) from inside the editor. The
language server ([Hyperion](LANGUAGE_SUPPORT.md)) tells ovim what is runnable at
the cursor and how to build and launch it; ovim builds, starts the program,
and shows everything in the **run console**.

## Keys and commands

| Keys | Command | Does |
|------|---------|------|
| `Space r r` / `Ctrl-F5` | `:Run` | run the code at the cursor (no debugger) |
| `Space r d` / `F5` / `Space d c` | `:Debug`, `:debug start` | debug the code at the cursor |
| `Space r l` / `Space d r` | `:RunLast`, `:debug last` | rerun the last run or debug (replaces a running one) |
| `Space r c` | `:RunConfig` | pick a configuration to run |
| `Space r C` / `Space d C` | `:DebugConfig` | pick a configuration to debug |
| `Space r s` / `Space d s` / `Shift-F5` | `:RunStop`, `:debug stop` | stop everything (build, program, debugger) |
| `Space r t` | `:RunConsole` | show / hide the run console |
| `Space r f` | `:RunFocus` | focus the console to scroll and jump |
| `Space r x` | `:RunClear` | drop finished runs from the console |
| `Space c l` / `Space c L` | `:CodeLens`, `:CodeLensDebug` | run / debug the code lens on this line |

While a debug session is stopped: `F5` continue, `F10` step over, `F11` step
in, `Shift-F11` step out, `F9` toggle breakpoint (also while running),
`Shift-F9` conditional breakpoint, `:eval expr`.

"The code at the cursor" is decided by the language server
(`hyperion.resolveLaunch`): inside a test method or class it is the test,
otherwise the `main` entry point of the enclosing class or file. If the server
finds nothing (or is too old to know), ovim falls back to the configurations
below: one configuration runs directly, several open a picker, none gives a
message telling you what to add.

## What happens on Run / Debug

1. **Save.** Modified buffers are written so the build sees what is on screen.
2. **Resolve.** The server returns the build command, classpath, main class
   (or test task), working directory and arguments.
3. **Build.** If the plan has a build step (`./gradlew :app:classes`,
   `mvn -q compile`, `javac -d ...`) it runs in the background; its output
   streams into the console and the editor stays responsive. Compiler errors
   from javac, kotlinc, Gradle and Maven land in the quickfix list with the
   right file, line and column (and javac's `symbol:`/`location:` details),
   the first error is opened, and the launch is **aborted**. `:make` uses the
   same parsers.
4. **Launch.** Run starts `java -cp ...` directly. Debug hands the plan to the
   debug adapter (DAP `launch`). Tests use the build tool: Run executes the
   test task and reads the JUnit XML reports afterwards (failures go to
   quickfix); Debug starts `--debug-jvm`, waits for
   `Listening for transport dt_socket at address: N` on stdout or stderr, and
   attaches to that port. It gives up after three minutes and shows the last
   output.

## The run console

A panel at the bottom of the editor (a "Run" tab in the GUI) keeps the output
of every run, with stdout, stderr, build output and editor notes in separate
colours. It stays after the process exits and shows the exit code and how long
it ran. It keeps the last eight runs.

`Space r f` focuses it: `j`/`k` move, `Ctrl-d`/`Ctrl-u` page, `g`/`G` top/bottom
(`G` follows live output again), `[` / `]` switch between runs, `r` rerun,
`s` stop, `x` clear, `q` back to the buffer. On a stack-trace line
(`at com.foo.Bar.baz(Bar.java:42)`) or a compiler error, `Enter` opens the
source. Frames are looked up under the project root (`src/main/java`,
`src/test/kotlin`, ...); JDK frames have no source and say so.

## `.ovim/debug.toml`

Project-local configurations, in the workspace root:

```toml
[[config]]
name = "App (build first)"
type = "launch"
main_class = "com.example.App"
classpath = "build/classes/java/main:build/resources/main"
args = ["--port", "8080"]
jvm_args = ["-Xmx512m"]
cwd = "."
build = ["gradle", "classes", "--console=plain"]   # optional; failure aborts

[[config]]
name = "Integration tests"
type = "gradle"                 # Run: gradle <task>; Debug: gradle <task> --debug-jvm
task = ":app:integrationTest"
args = ["--tests", "com.example.SlowIT"]

[[config]]
name = "Attach to server"
type = "attach"                 # debug only
host = "127.0.0.1"
port = 5005
```

Relative paths resolve against the workspace root of the current file (the
language server's root), not the directory ovim was started from. `./gradlew`
is used only when `gradle/wrapper/gradle-wrapper.jar` exists, otherwise
`gradle` from `PATH`. Configurations from `hyperion.runConfigurations` (for
example `.run/*.xml`) show up in the same picker. A malformed entry is
reported in the console instead of being skipped silently.

## Code lenses

Servers that provide `textDocument/codeLens` (Hyperion shows `▶ Run` on `main`
methods) get their lenses drawn at the end of the line as `▶ Run │ ▶ Debug`,
refreshed once edits settle and on `workspace/codeLens/refresh`. `Space c l`
on that line runs it. A `hyperion.run` lens goes through the same flow as
`Space r r` (a real JVM), not through Hyperion's built-in interpreter.

## Debug panels

While a session is active the side panel shows the call stack and variables
(`Space d v` toggles it, `Space d k`/`j` walk frames, `:DebugExpand name`
expands a variable). Debuggee output goes to the run console, and it is still
there after the program ends. When the session ends the execution marker is
cleared and the adapter process is stopped.
