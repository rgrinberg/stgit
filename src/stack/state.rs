use std::collections::BTreeMap;
use std::io::Write;
use std::str;

use chrono::{FixedOffset, TimeZone};
use git2::{Commit, FileMode, Oid, Tree};

use crate::error::Error;
use crate::patchname::PatchName;
use crate::stack::serde::RawStackState;
use crate::wrap::repository::commit_ex;
use crate::wrap::signature;

use super::iter::AllPatches;

pub(crate) struct StackState<'repo> {
    pub prev: Option<Commit<'repo>>,
    pub head: Commit<'repo>,
    pub applied: Vec<PatchName>,
    pub unapplied: Vec<PatchName>,
    pub hidden: Vec<PatchName>,
    pub patches: BTreeMap<PatchName, PatchDescriptor<'repo>>,
}

#[derive(Clone)]
pub(crate) struct PatchDescriptor<'repo> {
    pub commit: Commit<'repo>,
}

const MAX_PARENTS: usize = 16;

impl<'repo> StackState<'repo> {
    pub(super) fn new(head: Commit<'repo>) -> Self {
        Self {
            prev: None,
            head,
            applied: vec![],
            unapplied: vec![],
            hidden: vec![],
            patches: BTreeMap::new(),
        }
    }

    pub(super) fn from_tree(repo: &'repo git2::Repository, tree: &Tree) -> Result<Self, Error> {
        let stack_json = tree.get_name("stack.json");
        if let Some(stack_json) = stack_json {
            let stack_json_blob = repo.find_object(stack_json.id(), None)?.peel_to_blob()?;
            let raw_state = RawStackState::from_stack_json(stack_json_blob.content())?;
            Self::from_raw_state(repo, raw_state)
        } else {
            Err(Error::StackMetadataNotFound)
        }
    }

    fn from_raw_state(
        repo: &'repo git2::Repository,
        raw_state: RawStackState,
    ) -> Result<Self, Error> {
        let mut patches = BTreeMap::new();
        for (patchname, raw_desc) in raw_state.patches {
            let commit = repo.find_commit(raw_desc.oid)?;
            patches.insert(patchname, PatchDescriptor { commit });
        }
        Ok(Self {
            prev: if let Some(prev_id) = raw_state.prev {
                Some(repo.find_commit(prev_id)?)
            } else {
                None
            },
            head: repo.find_commit(raw_state.head)?,
            applied: raw_state.applied,
            unapplied: raw_state.unapplied,
            hidden: raw_state.hidden,
            patches,
        })
    }

    pub fn all_patches(&self) -> AllPatches<'_> {
        AllPatches::new(&self.applied, &self.unapplied, &self.hidden)
    }

    pub fn top(&self) -> &Commit<'repo> {
        if let Some(patch_name) = self.applied.last() {
            &self.patches[patch_name].commit
        } else {
            &self.head
        }
    }

    pub fn advance_head(self, new_head: Commit<'repo>, prev_state: Commit<'repo>) -> Self {
        Self {
            prev: Some(prev_state),
            head: new_head,
            ..self
        }
    }

    pub fn commit(
        &self,
        repo: &'repo git2::Repository,
        update_ref: Option<&str>,
        message: &str,
    ) -> Result<Oid, Error> {
        let prev_state_tree = match &self.prev {
            Some(prev_commit) => {
                let prev_tree = prev_commit.tree()?;
                let prev_state = Self::from_tree(repo, &prev_tree)?;
                Some((prev_state, prev_tree))
            }
            None => None,
        };
        let state_tree_id = self.make_tree(repo, &prev_state_tree)?;
        let config = repo.config()?; // TODO: wrapped config
        let sig = signature::default_committer(Some(&config))?;

        let simplified_parents: Vec<Oid> = match &self.prev {
            Some(prev_commit) => vec![prev_commit.parent_id(0)?],
            None => vec![],
        };

        let simplified_parent_id = commit_ex(
            repo,
            &sig,
            &sig,
            message,
            state_tree_id,
            &simplified_parents,
        )?;

        let mut parent_set = indexmap::IndexSet::new();
        parent_set.insert(self.head.id());
        parent_set.insert(self.top().id());
        for patch_name in &self.unapplied {
            parent_set.insert(self.patches[patch_name].commit.id());
        }
        for patch_name in &self.hidden {
            parent_set.insert(self.patches[patch_name].commit.id());
        }

        if let Some(prev_commit) = &self.prev {
            parent_set.insert(prev_commit.id());
            let (prev_state, _) = prev_state_tree.unwrap();
            for patch_name in prev_state.all_patches() {
                parent_set.remove(&prev_state.patches[patch_name].commit.id());
            }
        }

        let mut parent_oids: Vec<Oid> = parent_set.iter().copied().collect();

        while parent_oids.len() > MAX_PARENTS {
            let parent_group_oids: Vec<Oid> = parent_oids
                .drain(parent_oids.len() - MAX_PARENTS..parent_oids.len())
                .collect();
            let group_oid = commit_ex(
                repo,
                &sig,
                &sig,
                "parent grouping",
                state_tree_id,
                &parent_group_oids,
            )?;
            parent_oids.push(group_oid);
        }

        parent_oids.insert(0, simplified_parent_id);

        let commit_oid = commit_ex(repo, &sig, &sig, message, state_tree_id, &parent_oids)?;

        if let Some(refname) = update_ref {
            repo.reference(refname, commit_oid, true, message)?;
        }

        Ok(commit_oid)
    }

    fn make_tree(
        &self,
        repo: &'repo git2::Repository,
        prev_state_and_tree: &Option<(Self, Tree)>,
    ) -> Result<Oid, Error> {
        let mut builder = repo.treebuilder(None)?;
        builder.insert(
            "stack.json",
            repo.blob(serde_json::to_string_pretty(self)?.as_bytes())?,
            i32::from(FileMode::Blob),
        )?;

        let patches_tree_name = "patches";

        let (prev_state, prev_patches_tree) =
            if let Some((prev_state, prev_tree)) = prev_state_and_tree {
                let prev_patches_tree = prev_tree.get_name(patches_tree_name).and_then(|entry| {
                    entry
                        .to_object(repo)
                        .ok()
                        .and_then(|object| object.as_tree().map(|tree| tree.to_owned()))
                });
                (Some(prev_state), prev_patches_tree)
            } else {
                (None, None)
            };

        builder.insert(
            patches_tree_name,
            self.make_patches_tree(repo, prev_state, prev_patches_tree)?,
            i32::from(FileMode::Tree),
        )?;
        Ok(builder.write()?)
    }

    fn make_patches_tree(
        &self,
        repo: &git2::Repository,
        prev_state: Option<&StackState>,
        prev_patches_tree: Option<Tree>,
    ) -> Result<Oid, Error> {
        let mut builder = repo.treebuilder(None)?;
        for patch_name in self.all_patches() {
            builder.insert(
                patch_name.to_string(),
                self.make_patch_meta(repo, patch_name, prev_state, prev_patches_tree.as_ref())?,
                i32::from(FileMode::Blob),
            )?;
        }
        Ok(builder.write()?)
    }

    fn make_patch_meta(
        &self,
        repo: &git2::Repository,
        patch_name: &PatchName,
        prev_state: Option<&StackState>,
        prev_patches_tree: Option<&Tree>,
    ) -> Result<Oid, Error> {
        let commit = &self.patches[patch_name].commit;

        if let Some(prev_state) = prev_state {
            if let Some(prev_desc) = prev_state.patches.get(patch_name) {
                if prev_desc.commit.id() == commit.id() {
                    if let Some(prev_patches_tree) = prev_patches_tree {
                        if let Some(prev_patch_entry) =
                            prev_patches_tree.get_name(patch_name.as_ref())
                        {
                            if let Some(git2::ObjectType::Blob) = prev_patch_entry.kind() {
                                return Ok(prev_patch_entry.id());
                            }
                        }
                    }
                }
            }
        }

        let parent = commit.parent(0)?;
        let commit_time = commit.time();

        let commit_datetime = FixedOffset::east(commit_time.offset_minutes() * 60)
            .timestamp(commit_time.seconds(), 0);

        let mut patch_meta: Vec<u8> = Vec::with_capacity(1024);
        write!(
            patch_meta,
            "Bottom: {}\n\
             Top:    {}\n\
             Author: {}\n\
             Date:   {}\n\
             \n",
            parent.tree_id(),
            commit.tree_id(),
            commit.author(),
            commit_datetime.format("%Y-%m-%d %H:%M:%S %z"),
        )?;
        patch_meta.write_all(commit.message_raw_bytes())?;

        Ok(repo.blob(&patch_meta)?)
    }
}