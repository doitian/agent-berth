use anyhow::Result;

use crate::paths::Context;
use crate::providers::ProviderKind;
use crate::service;

pub fn setup(ctx: &Context, no_service: bool) -> Result<()> {
    if no_service {
        println!("skip service installation");
    } else {
        service::install(ctx)?;
    }
    for provider in ProviderKind::ALL {
        if !provider.is_installed(ctx) {
            println!("skip {} (not installed)", provider.name());
            continue;
        }
        match provider.install(ctx) {
            Ok(path) => println!("installed {} hooks: {}", provider.name(), path.display()),
            Err(err) => eprintln!("failed to install {}: {err:#}", provider.name()),
        }
    }
    if service::running(ctx) {
        println!("server is running");
    } else {
        eprintln!("server is not running yet; start it with: agent-bridge server");
    }
    Ok(())
}

pub fn teardown(ctx: &Context) -> Result<()> {
    service::uninstall(ctx)?;
    for provider in ProviderKind::ALL {
        match provider.uninstall(ctx) {
            Ok(()) => println!("removed {} hooks", provider.name()),
            Err(err) => eprintln!("failed to remove {}: {err:#}", provider.name()),
        }
    }
    Ok(())
}
