#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::{io, path::Path};

#[cfg(unix)]
use unix as sys;
#[cfg(windows)]
use windows as sys;

/// A builder used to create directories in various manners.
///
/// This builder also supports platform-specific options.
pub struct DirBuilder {
    recursive: bool,
    inner: sys::BuilderInner,
}

impl DirBuilder {
    /// Creates a new set of options with default mode/security settings for all
    /// platforms and also non-recursive.
    ///
    /// This an async version of [`std::fs::DirBuilder::new`]
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::DirBuilder;
    ///
    /// let builder = DirBuilder::new();
    /// ```
    pub fn new() -> Self {
        Self {
            recursive: false,
            inner: sys::BuilderInner::new(),
        }
    }

    /// Indicates that directories should be created recursively, creating all
    /// parent directories. Parents that do not exist are created with the same
    /// security and permissions settings.
    ///
    /// This option defaults to `false`.
    ///
    /// This an async version of [`std::fs::DirBuilder::recursive`]
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::DirBuilder;
    ///
    /// let mut builder = DirBuilder::new();
    /// builder.recursive(true);
    /// ```
    pub fn recursive(&mut self, recursive: bool) -> &mut Self {
        self.recursive = recursive;
        self
    }

    /// Creates the specified directory with the options configured in this
    /// builder.
    ///
    /// It is considered an error if the directory already exists unless
    /// recursive mode is enabled.
    ///
    /// This is async version of [`std::fs::DirBuilder::create`] and use io-uring
    /// in support platform.
    ///
    /// # Errors
    ///
    /// An error will be returned under the following circumstances:
    ///
    /// * Path already points to an existing file.
    /// * Path already points to an existing directory and the mode is non-recursive.
    /// * The calling process doesn't have permissions to create the directory or its missing
    ///   parents.
    /// * Other I/O error occurred.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use monoio::fs::DirBuilder;
    ///
    /// #[monoio::main]
    /// async fn main() -> std::io::Result<()> {
    ///     DirBuilder::new()
    ///         .recursive(true)
    ///         .create("/some/dir")
    ///         .await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub async fn create<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        if self.recursive {
            self.create_dir_all(path.as_ref()).await
        } else {
            self.inner.mkdir(path.as_ref()).await
        }
    }

    async fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        if path == Path::new("") {
            return Ok(());
        }

        let mut inexist_path = path;
        let mut need_create = vec![];

        while match self.inner.mkdir(inexist_path).await {
            Ok(()) => false,
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => true,
            Err(_) if is_dir(inexist_path).await => false,
            Err(e) => return Err(e),
        } {
            match inexist_path.parent() {
                Some(p) => {
                    need_create.push(inexist_path);
                    inexist_path = p;
                }
                None => return Err(io::Error::other("failed to create whole tree")),
            }
        }

        for p in need_create.into_iter().rev() {
            match self.inner.mkdir(p).await {
                Ok(()) => {}
                Err(_) if is_dir(p).await => {}
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }
}

impl Default for DirBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
impl DirBuilderExt for DirBuilder {
    fn mode(&mut self, mode: u32) -> &mut Self {
        self.inner.set_mode(mode);
        self
    }
}

// currently, will use the std version of metadata, will change to use the io-uring version
// when the statx is merge
async fn is_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}
