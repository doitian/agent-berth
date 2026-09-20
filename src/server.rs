use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use interprocess::local_socket::Stream;
use interprocess::local_socket::traits::ListenerExt;
use redb::Database;

use crate::db;
use crate::ipc;
use crate::paths::Context as AppContext;
use crate::protocol::{Request, Response};
use crate::store::Store;

pub fn run(ctx: &AppContext) -> Result<()> {
    if ipc::ping(ctx).is_ok() {
        anyhow::bail!("agent-berth server is already running");
    }

    let db = Arc::new(db::open(ctx)?);
    let mut store = db::load(&db)?;
    store.on_server_start();
    db::persist_heartbeats(&db, &store)?;
    std::fs::write(ctx.pid_path(), std::process::id().to_string())
        .with_context(|| format!("write {}", ctx.pid_path().display()))?;

    let listener = ipc::bind(ctx)?;
    let state = Arc::new(Mutex::new(store));
    let shutdown = Arc::new(AtomicBool::new(false));
    install_shutdown(ctx, shutdown.clone())?;

    let hb_state = state.clone();
    let hb_db = db.clone();
    let hb_ctx = ctx.clone();
    let hb_stop = shutdown.clone();
    thread::spawn(move || {
        while !hb_stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(5));
            if hb_stop.load(Ordering::Relaxed) {
                break;
            }
            if let Ok(mut store) = hb_state.lock() {
                persist_discover(&hb_db, &mut store, &hb_ctx);
                store.heartbeat();
                let _ = db::persist_heartbeats(&hb_db, &store);
            }
        }
    });

    eprintln!("listening on {}", ctx.endpoint_display());
    for conn in listener.incoming() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        match conn {
            Ok(stream) => {
                let state = state.clone();
                let db = db.clone();
                let ctx = ctx.clone();
                thread::spawn(move || {
                    if let Err(err) = handle(stream, &state, &db, &ctx) {
                        eprintln!("connection: {err:#}");
                    }
                });
            }
            Err(err) => eprintln!("accept: {err}"),
        }
    }
    cleanup(ctx);
    Ok(())
}

fn handle(stream: Stream, state: &Mutex<Store>, db: &Database, ctx: &AppContext) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        return Ok(());
    }
    let request: Request = serde_json::from_str(&line)?;
    let response = match request {
        Request::Ping => Response::ok(),
        Request::Notify { provider, payload } => match state.lock() {
            Ok(mut store) => match store.update(&provider, payload) {
                Ok(change) => {
                    if let Err(err) = db::persist_change(db, &store, &change) {
                        Response::error(err.to_string())
                    } else {
                        Response::ok()
                    }
                }
                Err(err) => Response::error(err.to_string()),
            },
            Err(_) => Response::error("store lock poisoned"),
        },
        Request::List { resumable, idle_ms } => match state.lock() {
            Ok(mut store) => {
                persist_discover(db, &mut store, ctx);
                let sessions = if resumable {
                    let idle = idle_ms
                        .map(Duration::from_millis)
                        .unwrap_or_else(crate::duration::default_idle);
                    store.resumable(idle)
                } else {
                    store.active()
                };
                Response::sessions(sessions)
            }
            Err(_) => Response::error("store lock poisoned"),
        },
    };
    let mut stream = reader.into_inner();
    let mut out = serde_json::to_vec(&response)?;
    out.push(b'\n');
    stream.write_all(&out)?;
    Ok(())
}

fn persist_discover(db: &Database, store: &mut Store, ctx: &AppContext) {
    if !store.discover(ctx) {
        return;
    }
    for provider in ["claude", "codex"] {
        let _ = db::persist_change(
            db,
            store,
            &crate::store::Change::Hooks {
                provider: provider.into(),
            },
        );
    }
}

fn install_shutdown(ctx: &AppContext, shutdown: Arc<AtomicBool>) -> Result<()> {
    let pid_path = ctx.pid_path();
    #[cfg(unix)]
    let sock = ctx.socket_path();
    ctrlc::set_handler(move || {
        shutdown.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_file(&pid_path);
        std::process::exit(0);
    })
    .context("install signal handler")?;
    Ok(())
}

fn cleanup(ctx: &AppContext) {
    let _ = std::fs::remove_file(ctx.pid_path());
    #[cfg(unix)]
    let _ = std::fs::remove_file(ctx.socket_path());
}
