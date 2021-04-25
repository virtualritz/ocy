use std::{collections::HashSet, path::PathBuf, sync::Arc};

use crate::{
    filesystem::{FileInfo, FileSystem, SimpleFileKind},
    matcher::Matcher,
};
use eyre::Report;
use eyre::Result;

pub struct Walker<FS: FileSystem, N: WalkNotifier> {
    fs: FS,
    matchers: Vec<Matcher>,
    notifier: N,
    ignores: HashSet<PathBuf>,
}

#[derive(Debug)]
pub struct RemovalCandidate {
    pub matcher_name: Arc<str>,
    pub file_info: FileInfo,
    pub file_size: Option<u64>,
}

impl RemovalCandidate {
    pub fn new(matcher_name: Arc<str>, file_info: FileInfo, file_size: Option<u64>) -> Self {
        Self {
            matcher_name,
            file_info,
            file_size,
        }
    }
}

pub trait WalkNotifier {
    fn notify_entered_directory(&self, dir: &FileInfo);
    fn notify_candidate_for_removal(&self, candidate: RemovalCandidate);
    fn notify_fail_to_scan(&self, e: &FileInfo, report: Report);
    fn notify_walk_finish(&self);
}

impl<FS: FileSystem, N: WalkNotifier> Walker<FS, N> {
    pub fn new(fs: FS, matchers: Vec<Matcher>, notifier: N, ignores: HashSet<PathBuf>) -> Self {
        Self {
            fs,
            matchers,
            notifier,
            ignores,
        }
    }

    pub fn walk_from_current_directory(&self) {
        let current = self.fs.current_directory().unwrap();

        self.process_dir(&current);
        self.notifier.notify_walk_finish();
    }

    fn process_dir(&self, file: &FileInfo) {
        if self.ignores.contains(&file.path) {
            return;
        }
        match self.process_entries(file) {
            Ok(children) => {
                children.iter().for_each(|d| self.process_dir(&d));
            }
            Err(report) => self.notifier.notify_fail_to_scan(file, report),
        }
    }

    fn process_entries(&self, file: &FileInfo) -> Result<Vec<FileInfo>> {
        self.notifier.notify_entered_directory(file);
        let mut entries = self.fs.list_files(&file)?;

        for matcher in &self.matchers {
            if matcher.any_entry_match(&entries) {
                let (mut to_remove, remaining) = matcher.find_files_to_remove(entries);
                to_remove.retain(|p| !self.ignores.contains(&p.path));
                self.notify_removal_candidates(matcher, to_remove);
                entries = remaining;
            }
        }
        entries.retain(|e| e.kind == SimpleFileKind::Directory && e.name != ".git");
        Ok(entries)
    }

    fn notify_removal_candidates(&self, matcher: &Matcher, to_remove: Vec<FileInfo>) {
        to_remove
            .into_iter()
            .map(|f| self.removal_candidate(matcher, f))
            .for_each(|c| self.notifier.notify_candidate_for_removal(c));
    }

    fn removal_candidate(&self, matcher: &Matcher, file: FileInfo) -> RemovalCandidate {
        let size = self.fs.file_size(&file).ok();
        RemovalCandidate::new(matcher.name.clone(), file, size)
    }
}
