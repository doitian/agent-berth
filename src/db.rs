use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::ipc;
use crate::paths::Context as AppContext;
use crate::status::AgentSession;
use crate::store::{Change, ListedSession, PluginSnapshot, ProviderStats, Store};

const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
const HOOKS: TableDefinition<&str, &[u8]> = TableDefinition::new("hooks");
const SNAPSHOTS: TableDefinition<&str, &[u8]> = TableDefinition::new("snapshots");
const REMOVED: TableDefinition<&str, u64> = TableDefinition::new("removed");

const LAST_HEARTBEAT: &str = "last_heartbeat_ms";
const PREV_HEARTBEAT: &str = "previous_heartbeat_ms";

pub fn open(ctx: &AppContext) -> Result<Database> {
    std::fs::create_dir_all(&ctx.state_dir)
        .with_context(|| format!("create {}", ctx.state_dir.display()))?;
    let db = Database::create(ctx.db_path())
        .with_context(|| format!("open {}", ctx.db_path().display()))?;
    init_tables(&db)?;
    migrate_json(&db, ctx)?;
    Ok(db)
}

pub fn load(db: &Database) -> Result<Store> {
    let txn = db.begin_read().context("read redb")?;
    let mut store = Store::default();
    if let Ok(table) = txn.open_table(META) {
        if let Some(value) = table.get(LAST_HEARTBEAT)? {
            store.last_heartbeat_ms = value.value();
        }
        if let Some(value) = table.get(PREV_HEARTBEAT)? {
            let ms = value.value();
            if ms > 0 {
                store.previous_heartbeat_ms = Some(ms);
            }
        }
    }
    if let Ok(table) = txn.open_table(HOOKS) {
        for entry in table.iter()? {
            let (key, value) = entry?;
            let Some((provider, sid)) = split_key(key.value()) else {
                continue;
            };
            let session: AgentSession = serde_json::from_slice(value.value())?;
            store
                .hooks
                .entry(provider.to_string())
                .or_default()
                .insert(sid.to_string(), session);
        }
    }
    if let Ok(table) = txn.open_table(SNAPSHOTS) {
        for entry in table.iter()? {
            let (key, value) = entry?;
            let Some((provider, instance)) = split_key(key.value()) else {
                continue;
            };
            let snapshot: PluginSnapshot = serde_json::from_slice(value.value())?;
            store
                .snapshots
                .entry(provider.to_string())
                .or_default()
                .insert(instance.to_string(), snapshot);
        }
    }
    if let Ok(table) = txn.open_table(REMOVED) {
        for entry in table.iter()? {
            let (key, value) = entry?;
            let Some((provider, session_id)) = split_key(key.value()) else {
                continue;
            };
            store
                .removed
                .entry(provider.to_string())
                .or_default()
                .insert(session_id.to_string(), value.value());
        }
    }
    Ok(store)
}

pub fn load_from_path(ctx: &AppContext) -> Result<Store> {
    let path = ctx.db_path();
    if !path.exists() {
        return load_legacy_json(ctx);
    }
    let db = Database::open(&path).with_context(|| format!("open {}", path.display()))?;
    load(&db)
}

pub fn query_sessions(
    ctx: &AppContext,
    resumable: bool,
    idle: Option<Duration>,
) -> Result<Vec<ListedSession>> {
    let idle_ms = idle.map(|d| d.as_millis() as u64);
    if let Ok(sessions) = ipc::list(ctx, resumable, idle_ms) {
        return Ok(sessions);
    }
    let mut store = load_from_path(ctx)?;
    store.discover(ctx);
    if resumable {
        Ok(store.resumable(idle))
    } else {
        Ok(store.active())
    }
}

pub fn query_all(ctx: &AppContext) -> Result<Vec<ListedSession>> {
    if let Ok(sessions) = ipc::list_all(ctx) {
        return Ok(sessions);
    }
    let mut store = load_from_path(ctx)?;
    store.discover(ctx);
    Ok(store.listed())
}

pub fn query_stats(ctx: &AppContext) -> Result<Vec<ProviderStats>> {
    if let Ok(stats) = ipc::stats(ctx) {
        return Ok(stats);
    }
    let mut store = load_from_path(ctx)?;
    store.discover(ctx);
    Ok(store.stats())
}

pub fn mark_removed(ctx: &AppContext, provider: &str, session_id: &str) -> Result<()> {
    let removed_at = crate::store::now_ms();
    if ipc::remove(ctx, provider, session_id).is_ok() {
        return Ok(());
    }
    let db = open(ctx)?;
    persist_removal(&db, provider, session_id, removed_at)
}

pub fn persist_heartbeats(db: &Database, store: &Store) -> Result<()> {
    let txn = db.begin_write().context("write redb")?;
    {
        let mut table = txn.open_table(META)?;
        table.insert(LAST_HEARTBEAT, store.last_heartbeat_ms)?;
        table.insert(PREV_HEARTBEAT, store.previous_heartbeat_ms.unwrap_or(0))?;
    }
    txn.commit()?;
    Ok(())
}

pub fn persist_change(db: &Database, store: &Store, change: &Change) -> Result<()> {
    match change {
        Change::Hooks { provider } => {
            let sessions = store.hooks.get(provider).cloned().unwrap_or_default();
            persist_hooks(db, provider, &sessions)
        }
        Change::Snapshot { provider, instance } => {
            let Some(snapshot) = store
                .snapshots
                .get(provider)
                .and_then(|bucket| bucket.get(instance))
            else {
                return Ok(());
            };
            persist_snapshot(db, provider, instance, snapshot)
        }
    }
}

fn persist_hooks(
    db: &Database,
    provider: &str,
    sessions: &BTreeMap<String, AgentSession>,
) -> Result<()> {
    let txn = db.begin_write().context("write redb")?;
    {
        let mut table = txn.open_table(HOOKS)?;
        let prefix = format!("{provider}\0");
        let mut stale = Vec::new();
        for entry in table.iter()? {
            let (key, _) = entry?;
            let key = key.value();
            if key.starts_with(&prefix) && !sessions.contains_key(&key[prefix.len()..]) {
                stale.push(key.to_string());
            }
        }
        for key in stale {
            table.remove(key.as_str())?;
        }
        for (sid, session) in sessions {
            let key = format!("{provider}\0{sid}");
            let bytes = serde_json::to_vec(session)?;
            table.insert(key.as_str(), bytes.as_slice())?;
        }
    }
    txn.commit()?;
    Ok(())
}

pub fn persist_removal(
    db: &Database,
    provider: &str,
    session_id: &str,
    removed_at_ms: u64,
) -> Result<()> {
    let txn = db.begin_write().context("write redb")?;
    {
        let mut table = txn.open_table(REMOVED)?;
        let key = format!("{provider}\0{session_id}");
        table.insert(key.as_str(), removed_at_ms)?;
    }
    txn.commit()?;
    Ok(())
}

fn persist_snapshot(
    db: &Database,
    provider: &str,
    instance: &str,
    snapshot: &PluginSnapshot,
) -> Result<()> {
    let txn = db.begin_write().context("write redb")?;
    {
        let mut table = txn.open_table(SNAPSHOTS)?;
        let key = format!("{provider}\0{instance}");
        let bytes = serde_json::to_vec(snapshot)?;
        table.insert(key.as_str(), bytes.as_slice())?;
    }
    txn.commit()?;
    Ok(())
}

pub fn persist_all(db: &Database, store: &Store) -> Result<()> {
    persist_heartbeats(db, store)?;
    sync_hooks(db, &store.hooks)?;
    sync_snapshots(db, &store.snapshots)?;
    sync_removed(db, &store.removed)?;
    Ok(())
}

fn sync_hooks(db: &Database, all: &BTreeMap<String, BTreeMap<String, AgentSession>>) -> Result<()> {
    let mut valid = std::collections::BTreeSet::new();
    for (provider, sessions) in all {
        for sid in sessions.keys() {
            valid.insert(format!("{provider}\0{sid}"));
        }
    }
    let txn = db.begin_write().context("write redb")?;
    {
        let mut table = txn.open_table(HOOKS)?;
        let mut stale = Vec::new();
        for entry in table.iter()? {
            let (key, _) = entry?;
            if !valid.contains(key.value()) {
                stale.push(key.value().to_string());
            }
        }
        for key in stale {
            table.remove(key.as_str())?;
        }
        for (provider, sessions) in all {
            for (sid, session) in sessions {
                let key = format!("{provider}\0{sid}");
                let bytes = serde_json::to_vec(session)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
        }
    }
    txn.commit()?;
    Ok(())
}

fn sync_snapshots(
    db: &Database,
    all: &BTreeMap<String, BTreeMap<String, PluginSnapshot>>,
) -> Result<()> {
    let mut valid = std::collections::BTreeSet::new();
    for (provider, instances) in all {
        for instance in instances.keys() {
            valid.insert(format!("{provider}\0{instance}"));
        }
    }
    let txn = db.begin_write().context("write redb")?;
    {
        let mut table = txn.open_table(SNAPSHOTS)?;
        let mut stale = Vec::new();
        for entry in table.iter()? {
            let (key, _) = entry?;
            if !valid.contains(key.value()) {
                stale.push(key.value().to_string());
            }
        }
        for key in stale {
            table.remove(key.as_str())?;
        }
        for (provider, instances) in all {
            for (instance, snapshot) in instances {
                let key = format!("{provider}\0{instance}");
                let bytes = serde_json::to_vec(snapshot)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
        }
    }
    txn.commit()?;
    Ok(())
}

fn sync_removed(db: &Database, all: &BTreeMap<String, BTreeMap<String, u64>>) -> Result<()> {
    let mut valid = std::collections::BTreeSet::new();
    for (provider, sessions) in all {
        for sid in sessions.keys() {
            valid.insert(format!("{provider}\0{sid}"));
        }
    }
    let txn = db.begin_write().context("write redb")?;
    {
        let mut table = txn.open_table(REMOVED)?;
        let mut stale = Vec::new();
        for entry in table.iter()? {
            let (key, _) = entry?;
            if !valid.contains(key.value()) {
                stale.push(key.value().to_string());
            }
        }
        for key in stale {
            table.remove(key.as_str())?;
        }
        for (provider, sessions) in all {
            for (sid, removed_at) in sessions {
                let key = format!("{provider}\0{sid}");
                table.insert(key.as_str(), *removed_at)?;
            }
        }
    }
    txn.commit()?;
    Ok(())
}

fn init_tables(db: &Database) -> Result<()> {
    let txn = db.begin_write().context("write redb")?;
    {
        let _ = txn.open_table(META)?;
        let _ = txn.open_table(HOOKS)?;
        let _ = txn.open_table(SNAPSHOTS)?;
        let _ = txn.open_table(REMOVED)?;
    }
    txn.commit()?;
    Ok(())
}

fn migrate_json(db: &Database, ctx: &AppContext) -> Result<()> {
    let json = ctx.json_legacy_path();
    if !json.exists() {
        return Ok(());
    }
    if has_heartbeat(db)? {
        let _ = std::fs::remove_file(&json);
        return Ok(());
    }
    let text =
        std::fs::read_to_string(&json).with_context(|| format!("read {}", json.display()))?;
    if !text.trim().is_empty() {
        let store: Store =
            serde_json::from_str(&text).with_context(|| format!("invalid {}", json.display()))?;
        persist_all(db, &store)?;
    }
    std::fs::remove_file(&json).ok();
    Ok(())
}

fn load_legacy_json(ctx: &AppContext) -> Result<Store> {
    let json = ctx.json_legacy_path();
    match std::fs::read_to_string(&json) {
        Ok(text) if text.trim().is_empty() => Ok(Store::default()),
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("invalid {}", json.display()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(err) => Err(err).with_context(|| format!("read {}", json.display())),
    }
}

fn has_heartbeat(db: &Database) -> Result<bool> {
    let txn = db.begin_read()?;
    let Ok(table) = txn.open_table(META) else {
        return Ok(false);
    };
    Ok(table.get(LAST_HEARTBEAT)?.is_some())
}

fn split_key(key: &str) -> Option<(&str, &str)> {
    key.split_once('\0')
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;
