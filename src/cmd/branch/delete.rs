// SPDX-License-Identifier: GPL-2.0-only

//! `stg branch --delete` implementation.

use anyhow::{anyhow, Result};

use crate::{
    branchloc::BranchLocator,
    ext::RepositoryExtended,
    stack::{InitializationPolicy, Stack, StackStateAccess},
};

pub(super) fn command() -> clap::Command {
    clap::Command::new("--delete")
        .override_usage(super::super::make_usage(
            "stg branch --delete",
            &["[--force] <branch>"],
        ))
        .about("Delete a branch")
        .long_about(
            "Delete a branch.\n\
             \n\
             The branch will not be deleted if there are any patches remaining unless \
             the '--force' option is provided.\n\
             \n\
             A protected branch may not be deleted; it must be unprotected first.",
        )
        .arg(
            clap::Arg::new("branch-any")
                .help("Branch to delete")
                .value_name("branch")
                .required(true)
                .value_parser(clap::value_parser!(BranchLocator)),
        )
        .arg(
            clap::Arg::new("force")
                .long("force")
                .help("Force deletion even if branch has patches")
                .action(clap::ArgAction::SetTrue),
        )
}

pub(super) fn dispatch(repo: &gix::Repository, matches: &clap::ArgMatches) -> Result<()> {
    let target_branch = matches
        .get_one::<BranchLocator>("branch-any")
        .expect("required argument")
        .resolve(repo)?;
    let target_branchname = target_branch.get_branch_partial_name()?;
    let current_branch = repo.get_current_branch().ok();
    let current_branchname = current_branch
        .as_ref()
        .and_then(|branch| branch.get_branch_partial_name().ok());
    if Some(target_branchname) == current_branchname {
        return Err(anyhow!("cannot delete the current branch"));
    }

    if let Ok(stack) = Stack::from_branch(
        repo,
        target_branch.clone(),
        InitializationPolicy::RequireInitialized,
    ) {
        if stack.is_protected(&repo.config_snapshot()) {
            return Err(anyhow!("delete not permitted: this branch is protected"));
        } else if !matches.get_flag("force") && stack.all_patches().count() > 0 {
            return Err(anyhow!(
                "delete not permitted: the series still contains patches (override with --force)"
            ));
        }
        stack.deinitialize()?;
    }

    target_branch.delete()?;
    Ok(())
}
