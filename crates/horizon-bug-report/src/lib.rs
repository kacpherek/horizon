//! Report Bugs from the pause menu (docs/telemetry.md, "Bug reports from the game"): a form that
//! writes the GitHub issue itself. The player writes what happened in a sentence and picks the
//! area; the game adds the version, the machine, the graphics adapter and its driver, the
//! settings and the end of the log, with the chat left out and the user name, e-mail and IP
//! addresses and IDs taken out. Preview shows the issue's title and saves everything that would
//! be sent to `bug-report.json` in the game folder; nothing leaves before Send. The telemetry
//! server opens the issue (`POST /v1/report`) with the label `from-game`.
//!
//! A Horizon crate: its page sits at the pause menu's Report Bugs icon (`Place::ReportBugs`),
//! which `horizon-official` registers with its hooks and lang file. Without it the icon opens
//! vanilla's link.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use horizon_platform::hooks::{Hooks, Started};
use horizon_platform::pages::{Act, Page, Place, Row};
use horizon_platform::system::{Machine, scrub::Scrubber};
use serde_json::{Value, json};

/// The form, for `horizon_platform::pages::register`.
pub static PAGE: ReportPage = ReportPage;
/// Its hooks (the version and the game folder), for `horizon_platform::hooks::register`.
pub static HOOKS: ReportHooks = ReportHooks;
/// Its text, for `horizon_platform::lang::register`.
pub const LANG: &horizon_platform::lang::Files =
    &[("en_us", include_str!("../assets/lang/en_us.json"))];

/// The areas a bug may be in: the repository's area label (none for Other), its button's key,
/// and the word the issue's title starts with.
const AREAS: [(&str, &str, &str); 11] = [
    ("", "horizon.report.area.other", "Bug"),
    ("blocks", "horizon.report.area.blocks", "Blocks"),
    ("mobs", "horizon.report.area.mobs", "Mobs"),
    ("rendering", "horizon.report.area.rendering", "Graphics"),
    ("audio", "horizon.report.area.audio", "Sound"),
    ("ui", "horizon.report.area.ui", "Menus"),
    (
        "multiplayer",
        "horizon.report.area.multiplayer",
        "Multiplayer",
    ),
    ("saves", "horizon.report.area.saves", "Saving"),
    (
        "worldgen",
        "horizon.report.area.worldgen",
        "World generation",
    ),
    ("chunks", "horizon.report.area.chunks", "Chunk loading"),
    ("commands", "horizon.report.area.commands", "Commands"),
];
/// The lines of the log's end a report carries, and its most characters (the server's limit).
const LOG_LINES: usize = 200;
const LOG_CHARS: usize = 40_000;
/// The server's limits on the player's sentences and on a table cell.
const SENTENCE: usize = 500;
const CELL: usize = 200;

/// The build and the game folder, from [`Hooks::started`].
static GAME: OnceLock<(String, PathBuf)> = OnceLock::new();

pub struct ReportHooks;

impl Hooks for ReportHooks {
    fn started(&self, game: &Started<'_>) {
        let _ = GAME.set((game.version.to_owned(), game.game_dir.to_owned()));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Write,
    Preview,
    Sending,
    Done,
}

struct Form {
    step: Step,
    happened: String,
    expected: String,
    area: usize,
    /// What Send sends, built by Preview.
    report: Option<Value>,
    /// After Send: the issue's link, or why it was not sent.
    answer: Result<Option<String>, String>,
}

static FORM: Mutex<Form> = Mutex::new(Form {
    step: Step::Write,
    happened: String::new(),
    expected: String::new(),
    area: 0,
    report: None,
    answer: Ok(None),
});

fn form() -> MutexGuard<'static, Form> {
    FORM.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct ReportPage;

impl Page for ReportPage {
    fn id(&self) -> &'static str {
        "horizon-bug-report.form"
    }
    fn title(&self) -> &'static str {
        "horizon.report.title"
    }
    fn place(&self) -> Place {
        Place::ReportBugs
    }
    /// A sent report's form starts over; one still sending shows how it went.
    fn opened(&self) {
        let mut form = form();
        match form.step {
            Step::Done => {
                *form = Form {
                    step: Step::Write,
                    happened: String::new(),
                    expected: String::new(),
                    area: 0,
                    report: None,
                    answer: Ok(None),
                }
            }
            Step::Preview => form.step = Step::Write,
            Step::Write | Step::Sending => {}
        }
    }
    fn rows(&self, rows: &mut Vec<Row>) {
        let form = form();
        match form.step {
            Step::Write => {
                rows.push(Row::Heading("horizon.report.hint"));
                rows.push(Row::field("horizon.report.happened", &*form.happened).max(SENTENCE));
                rows.push(Row::field("horizon.report.expected", &*form.expected).max(SENTENCE));
                rows.push(Row::button(AREAS[form.area].1).half());
                let ready = form.happened.trim().chars().count() >= 10;
                rows.push(Row::button("horizon.report.preview").half().active(ready));
            }
            Step::Preview => {
                let report = form.report.as_ref();
                let get = |key: &str| report.and_then(|r| r[key].as_str()).unwrap_or_default();
                rows.push(Row::Heading("horizon.report.public"));
                rows.push(Row::text(get("title")));
                rows.push(Row::text(summary(report)).quiet());
                rows.push(Row::Note(format!(
                    "Everything it sends: {}",
                    preview_file().display()
                )));
                rows.push(Row::button("horizon.report.send").half());
                rows.push(Row::button("gui.back").half());
            }
            Step::Sending => rows.push(Row::Heading("horizon.report.sending")),
            Step::Done => match &form.answer {
                Ok(Some(link)) => {
                    rows.push(Row::Heading("horizon.report.sent"));
                    rows.push(Row::text(link.clone()).groups());
                }
                Ok(None) => rows.push(Row::Heading("horizon.report.queued")),
                Err(why) => {
                    rows.push(Row::Heading("horizon.report.failed"));
                    rows.push(Row::text(why.clone()).quiet());
                }
            },
        }
    }
    fn act(&self, row: usize, act: Act) {
        let mut form = form();
        match (form.step, row, act) {
            (Step::Write, 1, Act::Edit(text)) => form.happened = text,
            (Step::Write, 2, Act::Edit(text)) => form.expected = text,
            (Step::Write, 3, Act::Press) => form.area = (form.area + 1) % AREAS.len(),
            (Step::Write, 3, Act::PressBack) => {
                form.area = (form.area + AREAS.len() - 1) % AREAS.len()
            }
            (Step::Write, 1 | 2, Act::Submit) | (Step::Write, 4, Act::Press)
                if form.happened.trim().chars().count() >= 10 =>
            {
                let report = report(&form);
                if let Err(e) = std::fs::write(
                    preview_file(),
                    serde_json::to_string_pretty(&report).unwrap_or_default(),
                ) {
                    log::warn!("Horizon: the bug report's preview file: {e}");
                }
                form.report = Some(report);
                form.step = Step::Preview;
            }
            (Step::Preview, 4, Act::Press) => {
                let Some(report) = form.report.clone() else {
                    return;
                };
                form.step = Step::Sending;
                let spawned =
                    std::thread::Builder::new()
                        .name("Bug report".into())
                        .spawn(move || {
                            let answer = send(&report);
                            let mut form = self::form();
                            form.answer = answer;
                            form.step = Step::Done;
                        });
                if let Err(e) = spawned {
                    form.answer = Err(e.to_string());
                    form.step = Step::Done;
                }
            }
            (Step::Preview, 5, Act::Press) => form.step = Step::Write,
            _ => {}
        }
    }
    fn back(&self) -> &'static str {
        match form().step {
            Step::Write | Step::Preview => "gui.cancel",
            Step::Sending | Step::Done => "gui.done",
        }
    }
}

/// Where Preview saves what Send would send.
fn preview_file() -> PathBuf {
    GAME.get()
        .map_or_else(PathBuf::new, |(_, dir)| dir.clone())
        .join("bug-report.json")
}

/// One grey line under the title: the build, the system and the graphics.
fn summary(report: Option<&Value>) -> String {
    let Some(report) = report else {
        return String::new();
    };
    let cell = |label: &str| {
        report["system"].as_array().and_then(|rows| {
            rows.iter()
                .find(|row| row[0] == label)
                .and_then(|row| row[1].as_str())
                .filter(|v| !v.is_empty())
        })
    };
    [report["version"].as_str(), cell("System"), cell("Graphics")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ")
}

/// The request `POST /v1/report` takes (crates/horizon-telemetry-server/src/game_report.rs).
fn report(form: &Form) -> Value {
    let (version, dir) = GAME.get().map_or(("unknown", Path::new("")), |(v, d)| {
        (v.as_str(), d.as_path())
    });
    let scrub = Scrubber::new();
    let happened = scrub.scrub(form.happened.trim());
    let (area, _, word) = AREAS[form.area];
    let first: String = happened
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(100)
        .collect();
    let (system, settings) = tables(dir);
    json!({
        "category": "bug",
        "title": format!("{word}: {first}"),
        "happened": happened,
        "expected": scrub.scrub(form.expected.trim()),
        "area": area,
        "version": version,
        "system": system,
        "settings": settings,
        "log": log_end(dir, &scrub),
    })
}

/// The Machine and Settings tables: what `horizon-system` reads of this machine, and what the
/// game says of its graphics and settings (`horizon_platform::details`). An empty value reads
/// "not given by the system" in the issue.
fn tables(dir: &Path) -> (Vec<[String; 2]>, Vec<[String; 2]>) {
    let m = Machine::detect_in(Some(dir).filter(|d| !d.as_os_str().is_empty()));
    let mhz = |n: u32| (n > 0).then(|| format!("{:.1} GHz", f64::from(n) / 1000.0));
    let clocks = match (mhz(m.cpu_base_mhz), mhz(m.cpu_boost_mhz)) {
        (Some(base), Some(boost)) => format!(", base {base}, boost {boost}"),
        (Some(one), None) | (None, Some(one)) => format!(", {one}"),
        (None, None) => String::new(),
    };
    let os = if m.os_version.is_empty() {
        &m.os
    } else {
        &m.os_version
    };
    let mut system = vec![
        ["System".into(), format!("{os} ({})", m.arch)],
        [
            "CPU".into(),
            format!(
                "{}, {} cores, {} threads{clocks}",
                m.cpu, m.cores, m.threads
            ),
        ],
        ["Memory".into(), format!("{} GB", m.ram_gb)],
        ["Drive".into(), m.storage.into()],
        ["Power".into(), [m.form, m.power].join(" ").trim().into()],
    ];
    let mut settings = Vec::new();
    for (key, value) in horizon_platform::details::all() {
        match key {
            "Graphics Adapter" => system.push(["Graphics".into(), value]),
            "DirectX 12 shader compiler" => system.push(["DX12 shader compiler".into(), value]),
            "Native Mods" => system.push(["Native mods".into(), value]),
            "Resource Packs" => settings.push(["Resource packs".into(), value]),
            // "render distance 12, graphics fancy, VSync ON, ...": one row each.
            "Render Settings" => settings.extend(value.split(", ").map(|part| {
                let (label, value) = part.rsplit_once(' ').unwrap_or((part, ""));
                [label.to_owned(), value.to_owned()]
            })),
            _ => {}
        }
    }
    let fit = |rows: Vec<[String; 2]>| {
        rows.into_iter()
            .map(|[label, value]| {
                let label: String = label.chars().take(40).collect();
                [label, value.chars().take(CELL).collect()]
            })
            .collect::<Vec<_>>()
    };
    (fit(system), fit(settings))
}

/// The end of `logs/latest.log`: the last lines without the chat, private text taken out, cut
/// from the start to the server's limit.
fn log_end(dir: &Path, scrub: &Scrubber) -> String {
    let Ok(text) = std::fs::read_to_string(dir.join("logs").join("latest.log")) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.contains("[CHAT]")).collect();
    let kept = scrub.scrub(&lines[lines.len().saturating_sub(LOG_LINES)..].join("\n"));
    let skip = kept.chars().count().saturating_sub(LOG_CHARS);
    kept.chars().skip(skip).collect()
}

/// Sends the report: the issue's link, None when the server queued it, or why it failed.
fn send(report: &Value) -> Result<Option<String>, String> {
    let endpoint = horizon_telemetry::endpoint()
        .ok_or("Sending is turned off on this computer (HORIZON_TELEMETRY_URL).")?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    let answer = agent
        .post(&format!("{endpoint}/v1/report"))
        .set("Content-Type", "application/json")
        .send_string(&report.to_string());
    let read = |response: ureq::Response| -> Value {
        response
            .into_string()
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    };
    match answer {
        Ok(response) => Ok(read(response)["issue"].as_str().map(str::to_owned)),
        Err(ureq::Error::Status(429, _)) => Err(
            "Too many reports today. Try again tomorrow, or post it on Discord in #horizon-bugs."
                .into(),
        ),
        Err(ureq::Error::Status(code, response)) => {
            let body = read(response);
            let why = body["error"].as_str().unwrap_or("no reason given");
            Err(format!("The server answered {code}: {why}"))
        }
        Err(e) => Err(format!("The server did not answer: {e}")),
    }
}
