use anyhow::Result;

pub(crate) trait TemporaryIndex {
    fn with_temp_index<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut git2::Index) -> Result<T>;

    fn with_temp_index_file<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut git2::Index) -> Result<T>;
}

impl TemporaryIndex for git2::Repository {
    fn with_temp_index<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut git2::Index) -> Result<T>,
    {
        let mut temp_index = git2::Index::new()?;
        let mut orig_index = self.index()?;
        self.set_index(&mut temp_index)?;
        let result = f(&mut temp_index);
        self.set_index(&mut orig_index)
            .expect("can reset to original index");
        result
    }

    fn with_temp_index_file<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut git2::Index) -> Result<T>,
    {
        // TODO: dynamic file name
        let temp_index_path = self.path().join("index-temp-stgit");
        let mut temp_index = git2::Index::open(&temp_index_path)?;
        let mut orig_index = self.index()?;
        temp_index.write()?;
        self.set_index(&mut temp_index)?;
        let result = f(&mut temp_index);
        self.set_index(&mut orig_index)
            .expect("can reset to original index");
        std::fs::remove_file(&temp_index_path).expect("can remove temp index file");
        result
    }
}
