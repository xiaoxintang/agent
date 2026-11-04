use std::{
    fs::{self, File},
    io,
    path::Path,
};

pub fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    check_private(path, true)
}

pub fn private_file(path: &Path) -> io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    match options.open(path) {
        Ok(file) => {
            check_file(&file)?;
            Ok(file)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            check_private(path, false)?;
            let mut existing = fs::OpenOptions::new();
            existing.read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                existing.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
            }
            let file = existing.open(path)?;
            check_file(&file)?;
            Ok(file)
        }
        Err(error) => Err(error),
    }
}

fn check_private(path: &Path, directory: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(io::Error::other(
            "Data path must be a directory or regular file, not a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Data directory/database must not be accessible to group or other users",
            ));
        }
    }
    Ok(())
}

fn check_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("Private data must be a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Private file owner or permissions are invalid",
            ));
        }
    }
    Ok(())
}
pub fn disable_core_dumps() -> io::Result<()> {
    #[cfg(unix)]
    rustix::process::setrlimit(
        rustix::process::Resource::Core,
        rustix::process::Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )?;
    Ok(())
}
