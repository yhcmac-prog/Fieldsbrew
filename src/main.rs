mod archive;
mod bottle;
mod build;
mod cask;
mod cellar;
mod cli;
mod commands;
mod config;
mod convert;
mod deps;
mod download;
mod formula;
mod homebrew;
mod link;
mod macports;
mod platform;
mod sandbox;
mod sign;
mod version;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};
use config::Config;

fn main() {
    if let Err(e) = run() {
        eprintln!("fb: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::new(cli.verbose);
    match cli.command {
        Command::Install { names, no_sandbox, keep_tmp } => {
            platform::ensure_supported()?;
            commands::install::run(
                &cfg,
                &names,
                commands::install::InstallOpts { no_sandbox, keep_tmp },
            )
        }
        Command::Uninstall { names, force } => {
            platform::ensure_supported()?;
            commands::uninstall::run(&cfg, &names, force)
        }
        Command::Upgrade {
            names,
            no_sandbox,
            keep_tmp,
        } => {
            platform::ensure_supported()?;
            commands::upgrade::run(
                &cfg,
                &names,
                commands::install::InstallOpts { no_sandbox, keep_tmp },
            )
        }
        Command::Reinstall {
            names,
            no_sandbox,
            keep_tmp,
        } => {
            platform::ensure_supported()?;
            commands::reinstall::run(
                &cfg,
                &names,
                commands::install::InstallOpts { no_sandbox, keep_tmp },
            )
        }
        Command::Outdated => {
            platform::ensure_supported()?;
            commands::outdated::run(&cfg)
        }
        Command::Update => commands::update::run(&cfg),
        Command::Search { query } => {
            platform::ensure_supported()?;
            commands::search::run(&cfg, query.as_deref())
        }
        Command::Info { name } => {
            platform::ensure_supported()?;
            commands::info::run(&cfg, &name)
        }
        Command::List { brew } => {
            platform::ensure_supported()?;
            commands::list::run(&cfg, brew)
        }
        Command::Deps { name, tree } => {
            platform::ensure_supported()?;
            commands::deps::run(&cfg, &name, tree)
        }
        Command::Uses { name } => {
            platform::ensure_supported()?;
            commands::uses::run(&cfg, &name)
        }
        Command::Link { names, force } => {
            platform::ensure_supported()?;
            commands::linking::link_names(&cfg, &names, force)
        }
        Command::Unlink { names } => {
            platform::ensure_supported()?;
            commands::linking::unlink_names(&cfg, &names)
        }
        Command::Tap { name, url, remove, core } => {
            commands::tap::run(&cfg, name.as_deref(), url.as_deref(), remove, core)
        }
        Command::Shellenv { shell } => commands::shellenv::run(&cfg, shell.as_deref()),
        Command::Convert { path, out, cask } => {
            commands::convert::run(&cfg, &path, out.as_deref(), cask)
        }
        Command::Cask { command } => {
            platform::ensure_supported()?;
            commands::cask::run(&cfg, command)
        }
        Command::Sign {
            file,
            key_file,
            output,
        } => commands::sign::run_sign(&file, key_file.as_deref(), output.as_deref()),
        Command::Verify {
            file,
            signature,
            public_key,
        } => commands::sign::run_verify(&file, signature.as_deref(), public_key.as_deref()),
        Command::Net { timeout } => commands::net::run(timeout),
        Command::SelfUpdate {
            check,
            url,
            repo,
            version,
            force,
            sha256,
            public_key,
        } => {
            platform::ensure_supported()?;
            commands::self_update::run(commands::self_update::Options {
                check,
                url,
                repo,
                version,
                force,
                sha256,
                public_key,
            })
        }
        Command::Services { command } => {
            use cli::ServicesCommand;
            use commands::services::Action;
            let (action, name) = match command {
                None => (Action::List, None),
                Some(ServicesCommand::Run { name }) => (Action::Run, Some(name)),
                Some(ServicesCommand::Start { name }) => (Action::Start, Some(name)),
                Some(ServicesCommand::Stop { name }) => (Action::Stop, Some(name)),
                Some(ServicesCommand::Restart { name }) => (Action::Restart, Some(name)),
                Some(ServicesCommand::List) => (Action::List, None),
                Some(ServicesCommand::Cleanup) => (Action::Cleanup, None),
            };
            commands::services::run(&cfg, action, name.as_deref())
        }
        Command::Migrate {
            names,
            no_sandbox,
            keep_tmp,
        } => {
            platform::ensure_supported()?;
            commands::migrate::run(
                &cfg,
                &names,
                commands::install::InstallOpts {
                    no_sandbox,
                    keep_tmp,
                },
            )
        }
        Command::Audit { names } => commands::audit::run(&cfg, &names),
        Command::Bottle {
            names,
            out,
            skip_build,
            keep_tmp,
            no_sandbox,
        } => commands::bottle::run(
            &cfg,
            &names,
            out.as_deref(),
            skip_build,
            keep_tmp,
            no_sandbox,
            cfg.verbose,
        ),
        Command::Cleanup => {
            platform::ensure_supported()?;
            commands::cleanup::run(&cfg)
        }
        // doctor 负责自诊断，不在此处做平台拦截
        Command::Doctor => commands::doctor::run(&cfg),
    }
}
