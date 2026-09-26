use std::io::{self, Write};

use anyhow::Result;

use crate::db;
use crate::paths::Context;
use crate::providers::ProviderKind;
use crate::providers::opencode;
use crate::service;
use crate::tmux;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    Warn,
    Error,
    Skip,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Skip => "skip",
        }
    }
}

struct Check {
    level: Level,
    name: &'static str,
    detail: String,
}

pub fn run(ctx: &Context) -> Result<()> {
    let checks = collect(ctx);
    print_checks(&checks)?;
    if checks.iter().any(|check| check.level == Level::Error) {
        anyhow::bail!("doctor found issues");
    }
    Ok(())
}

fn collect(ctx: &Context) -> Vec<Check> {
    let mut checks = vec![
        binary(ctx),
        server(ctx),
        service_check(ctx),
        state(ctx),
        tmux_check(),
        fzf_check(),
    ];
    for provider in ProviderKind::ALL {
        checks.push(provider_check(ctx, provider));
    }
    checks
}

fn binary(ctx: &Context) -> Check {
    let path = &ctx.berth_bin;
    if path.is_file() {
        Check {
            level: Level::Ok,
            name: "binary",
            detail: path.display().to_string(),
        }
    } else {
        Check {
            level: Level::Error,
            name: "binary",
            detail: format!("{} not found", path.display()),
        }
    }
}

fn server(ctx: &Context) -> Check {
    let endpoint = ctx.endpoint_display();
    if service::running(ctx) {
        Check {
            level: Level::Ok,
            name: "server",
            detail: format!("running on {endpoint}"),
        }
    } else {
        Check {
            level: Level::Error,
            name: "server",
            detail: format!("not running on {endpoint} (start with: agent-berth server)"),
        }
    }
}

fn service_check(ctx: &Context) -> Check {
    if service::installed(ctx) {
        Check {
            level: Level::Ok,
            name: "service",
            detail: service_label().into(),
        }
    } else {
        Check {
            level: Level::Warn,
            name: "service",
            detail: format!("{} not installed (run: agent-berth setup)", service_label()),
        }
    }
}

fn service_label() -> &'static str {
    #[cfg(windows)]
    {
        "scheduled task AgentBerth"
    }
    #[cfg(unix)]
    {
        "systemd user unit agent-berth.service"
    }
}

fn state(ctx: &Context) -> Check {
    let path = ctx.db_path();
    if service::running(ctx) {
        return Check {
            level: Level::Ok,
            name: "state",
            detail: format!("{} (held by server)", path.display()),
        };
    }
    if !path.exists() {
        return Check {
            level: Level::Warn,
            name: "state",
            detail: format!("{} missing", path.display()),
        };
    }
    match db::load_from_path(ctx) {
        Ok(store) => {
            let sessions = store.listed().len();
            Check {
                level: Level::Ok,
                name: "state",
                detail: format!(
                    "{} ({sessions} session{})",
                    path.display(),
                    if sessions == 1 { "" } else { "s" }
                ),
            }
        }
        Err(err) => Check {
            level: Level::Error,
            name: "state",
            detail: format!("cannot read {}: {err:#}", path.display()),
        },
    }
}

fn tmux_check() -> Check {
    if tmux::available() {
        Check {
            level: Level::Ok,
            name: "tmux",
            detail: "found".into(),
        }
    } else {
        Check {
            level: Level::Warn,
            name: "tmux",
            detail: "not on PATH (needed to resume CLI sessions)".into(),
        }
    }
}

fn fzf_check() -> Check {
    if crate::paths::on_path("fzf") {
        Check {
            level: Level::Ok,
            name: "fzf",
            detail: "found".into(),
        }
    } else {
        Check {
            level: Level::Warn,
            name: "fzf",
            detail: "not on PATH (needed to attach to agents)".into(),
        }
    }
}

fn provider_check(ctx: &Context, provider: ProviderKind) -> Check {
    let name = provider.name();
    if !provider.is_installed(ctx) {
        return Check {
            level: Level::Skip,
            name,
            detail: "not installed".into(),
        };
    }
    if !provider.hooks_installed(ctx) {
        let mismatch = if matches!(provider, ProviderKind::Opencode) {
            opencode::version_mismatch(ctx)
        } else {
            None
        };
        if let Some(detail) = mismatch {
            return Check {
                level: Level::Error,
                name,
                detail: format!("{detail} (run: agent-berth setup --no-service)"),
            };
        }
        return Check {
            level: Level::Error,
            name,
            detail: format!(
                "hooks missing ({}) (run: agent-berth setup --no-service)",
                provider.hook_path(ctx).display()
            ),
        };
    }
    if provider.hooks_stale(ctx) {
        return Check {
            level: Level::Warn,
            name,
            detail: format!(
                "hooks point at a different binary ({})",
                provider.hook_path(ctx).display()
            ),
        };
    }
    if provider.hooks_outdated(ctx) {
        return Check {
            level: Level::Warn,
            name,
            detail: format!(
                "hooks out of date ({}) (run: agent-berth setup --no-service)",
                provider.hook_path(ctx).display()
            ),
        };
    }
    Check {
        level: Level::Ok,
        name,
        detail: format!("hooks installed ({})", provider.hook_path(ctx).display()),
    }
}

fn print_checks(checks: &[Check]) -> Result<()> {
    let name_width = checks
        .iter()
        .map(|check| check.name.len())
        .max()
        .unwrap_or(0);
    let mut out = io::stdout().lock();
    for check in checks {
        writeln!(
            out,
            "{:<5}  {:name_width$}  {}",
            check.level.label(),
            check.name,
            check.detail,
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
