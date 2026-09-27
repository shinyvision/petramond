use std::path::PathBuf;
use std::sync::Arc;

use super::{ScreenCtx, ShellCommand};
use crate::app::content::ContentSession;
use crate::app::launched::launch_entries;
use crate::app::{AppScreen, ExitKind};
use petramond_ui::{NavKey, UiEvent, UiMap, UiState, UiValue};

const LAUNCHERS: &str = "launchers";
const LAUNCHER: &str = "launcher";

fn icon_name(pack_id: &str) -> String {
    format!("launch_icon:{pack_id}")
}

pub(super) fn prepare(ctx: &mut ScreenCtx) -> bool {
    let icons: Vec<(String, PathBuf)> = launch_entries()
        .map(|(id, entry)| (icon_name(id), entry.icon.clone()))
        .collect();
    ctx.ui.set_extra_images(&icons);
    true
}

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let rows: Vec<UiMap> = launch_entries()
        .map(|(id, _)| {
            let mut row = UiMap::new();
            row.insert("icon".into(), UiValue::Str(icon_name(id)));
            row
        })
        .collect();
    state.set("has_launchers", UiValue::Bool(!rows.is_empty()));
    state.set(LAUNCHERS, UiValue::List(Arc::new(rows)));
    let tip = ctx
        .ui
        .hover_item(LAUNCHERS)
        .and_then(|index| launch_entries().nth(index).map(|(_, entry)| entry))
        .map(|entry| entry.label.clone())
        .unwrap_or_default();
    state.set("launch_tip", UiValue::Str(tip));
    let (notice, notice_tip) = notice(ctx.content_report, ctx.content);
    state.set("has_notice", UiValue::Bool(!notice.is_empty()));
    state.set("notice", UiValue::Str(notice));
    state.set("notice_tip", UiValue::Str(notice_tip));
    state.set(
        "account_tip",
        UiValue::Str(match &ctx.shell.account.saved {
            Some(saved) => format!("Account: {}", saved.username),
            None => "Sign in".to_owned(),
        }),
    );
}

pub(in crate::app) fn notice(
    report: &petramond::content::ApplyReport,
    content: &ContentSession,
) -> (String, String) {
    if let Some((dir, _)) = report.failed.first() {
        let line = match report.failed.len() {
            1 => format!("{dir} could not be installed"),
            n => format!("{n} content changes could not be applied"),
        };
        let tip = report
            .failed
            .iter()
            .map(|(dir, why)| format!("{dir}: {why}"))
            .collect::<Vec<_>>()
            .join("\n");
        return (line, tip);
    }
    if report.deferred {
        let line = "Close other Petramond windows to install".to_owned();
        return (line.clone(), line);
    }
    let jobs = &content.jobs;
    if !jobs.is_empty() {
        let (done, total) = jobs
            .iter()
            .map(|j| j.bytes())
            .fold((0, 0), |(d, t), (jd, jt)| (d + jd, t + jt));
        let percent = (done * 100).checked_div(total).unwrap_or(0);
        let items = if jobs.len() == 1 { "item" } else { "items" };
        let line = format!("Downloading {} {items} · {percent}%", jobs.len());
        return (line.clone(), line);
    }
    if !content.pending.is_empty() {
        let line = "Apply content changes in Content".to_owned();
        return (line.clone(), line);
    }
    (String::new(), String::new())
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, item, .. } => match id.as_str() {
            "start" => start(ctx),
            "connect" => ctx.request(ShellCommand::OpenConnectServer),
            "browse" => ctx.request(ShellCommand::OpenContent {
                back: None,
                filter: None,
            }),
            "account" => ctx.request(ShellCommand::OpenAccount(None)),
            "options" => ctx.request(ShellCommand::Push(AppScreen::Options)),
            "quit" => ctx.request(ShellCommand::Exit(ExitKind::Quit)),
            LAUNCHER => {
                if let Some(index) = item {
                    launch(ctx, index as usize);
                }
            }
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => start(ctx),
        _ => {}
    }
}

fn launch(ctx: &mut ScreenCtx, index: usize) {
    let Some((pack_id, _)) = launch_entries().nth(index) else {
        return;
    };
    let screen = ctx
        .ui
        .frame_stamp()
        .map_or((0, 0), |(_, viewport)| viewport.size);
    ctx.request(ShellCommand::LaunchPack {
        pack_id: pack_id.to_owned(),
        screen,
    });
}

fn start(ctx: &mut ScreenCtx) {
    ctx.shell.refresh_worlds();
    ctx.shell.select_world(None);
    ctx.goto(AppScreen::WorldSelect);
}
