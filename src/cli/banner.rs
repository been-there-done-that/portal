use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

/// Print the Go Fiber-style startup banner after the child process is spawned.
///
/// ```text
///   portal  v1.0.0  ·  ● running
///
///   https://myapp.localhost
///   └─ localhost:4123  ·  cert ✓  ·  pid 91842
/// ```
pub fn print_banner(hostname: &str, port: u16, pid: u32, replaced: bool) {
    let version = env!("CARGO_PKG_VERSION");
    let badge = style(" portal ").bold().white().on_blue();
    let ver = style(format!("v{version}")).dim();
    let status_dot = if replaced {
        style("● replaced").yellow().to_string()
    } else {
        style("● running").green().to_string()
    };
    eprintln!("  {badge}  {ver}  ·  {status_dot}");
    eprintln!();
    eprintln!("  {}", style(format!("https://{hostname}")).bold().white());
    eprintln!(
        "  {}{}  ·  {}  ·  {}",
        style("└─ localhost:").dim(),
        style(port.to_string()).red(),
        style("cert ✓").green(),
        style(format!("pid {pid}")).dim(),
    );
}

/// Manages animated setup steps printed before the first run of a project.
///
/// Steps are shown as a tree with `indicatif` spinners:
/// ```text
///   portal  v1.0.0  ·  first run
///
///   ├─ cert    generating…
///   ├─ daemon  starting…
///   ╰─ ready
/// ```
pub struct SetupPrinter {
    mp: MultiProgress,
    started: bool,
}

impl SetupPrinter {
    pub fn new() -> Self {
        Self {
            mp: MultiProgress::new(),
            started: false,
        }
    }

    /// Print the header line once, on the first step.
    fn ensure_header(&mut self) {
        if !self.started {
            self.started = true;
            let version = env!("CARGO_PKG_VERSION");
            let badge = style(" portal ").bold().white().on_blue();
            let label = style(format!("v{version}  ·  first run")).dim();
            let _ = self.mp.println(format!("  {badge}  {label}"));
            let _ = self.mp.println("");
        }
    }

    /// Add an animated spinner for a setup step. Returns a handle to finish it.
    ///
    /// ```rust
    /// let pb = setup.begin_step("daemon", "starting…");
    /// // ... run async work ...
    /// pb.finish_with_message(format!("{} daemon  started", console::style("✓").green()));
    /// ```
    pub fn begin_step(&mut self, name: &str, msg: &str) -> ProgressBar {
        self.ensure_header();
        let pb = self.mp.add(ProgressBar::new_spinner());
        let spinner_style = ProgressStyle::with_template("  {spinner:.cyan} {msg}")
            .expect("invalid spinner template")
            .tick_strings(&[
                "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏",
            ]);
        pb.set_style(spinner_style);
        pb.set_message(format!("{:<8} {}", name, msg));
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb
    }

    /// Print the `╰─ ready` footer and clear the MultiProgress.
    /// No-op if no steps were started (nothing to display).
    pub fn done(self) {
        if self.started {
            eprintln!("  {}", style("╰─ ready").dim());
            eprintln!();
        }
    }
}

impl Default for SetupPrinter {
    fn default() -> Self {
        Self::new()
    }
}
