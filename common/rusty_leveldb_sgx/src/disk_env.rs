use crate::env::{path_to_str, Env, FileLock, Logger, RandomAccess};
use crate::env_common::micros;
use crate::error::{err, Result, Status, StatusCode};

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::io::{Seek, SeekFrom};
use std::iter::FromIterator;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::untrusted::fs;
use std::untrusted::path::PathEx;

pub type DBPersistKey = [u8; 16];

const F_RDLCK: libc::c_short = 0;
const F_WRLCK: libc::c_short = 1;
const F_UNLCK: libc::c_short = 2;

type FileDescriptor = i32;

#[derive(Clone)]
pub struct PosixDiskEnv {
    locks: Arc<Mutex<HashMap<String, sgx_tprotected_fs::SgxFile>>>,
    key: DBPersistKey,
}

impl PosixDiskEnv {
    pub fn new_with(key: DBPersistKey) -> PosixDiskEnv {
        PosixDiskEnv {
            locks: Arc::new(Mutex::new(HashMap::new())),
            key,
        }
    }
}

/// map_err_with_name annotates an io::Error with information about the operation and the file.
fn map_err_with_name(method: &'static str, f: &Path, e: io::Error) -> Status {
    let mut s = Status::from(e);
    s.err = format!("{}: {}: {}", method, s.err, path_to_str(f));
    s
}

// Note: We're using Ok(f()?) in several locations below in order to benefit from the automatic
// error conversion using std::convert::From.
impl Env for PosixDiskEnv {
    fn open_sequential_file(&self, p: &Path) -> Result<Box<dyn Read>> {
        Ok(Box::new(
            sgx_tprotected_fs::OpenOptions::default()
                .read(true)
                .open_with_key(p, self.key)
                .map_err(|e| map_err_with_name("open_sgx (seq)", p, e))?,
        ))
    }
    fn open_random_access_file(&self, p: &Path) -> Result<Box<dyn RandomAccess>> {
        Ok(sgx_tprotected_fs::OpenOptions::default()
            .read(true)
            .open_with_key(p, self.key)
            .map(|f| {
                let b: Box<dyn RandomAccess> = Box::new(f);
                b
            })
            .map_err(|e| map_err_with_name("open_sgx (randomaccess)", p, e))?)
    }
    fn open_writable_file(&self, p: &Path) -> Result<Box<dyn Write>> {
        Ok(Box::new(
            sgx_tprotected_fs::OpenOptions::default()
                .write(true)
                .append(false)
                .open_with_key(p, self.key)
                .map_err(|e| map_err_with_name("open_sgx (write)", p, e))?,
        ))
    }
    fn open_appendable_file(&self, p: &Path) -> Result<Box<dyn Write>> {
        Ok(Box::new(
            sgx_tprotected_fs::OpenOptions::default()
                .append(true)
                .open_with_key(p, self.key)
                .map_err(|e| map_err_with_name("open_sgx (append_sgx)", p, e))?,
        ))
    }
    fn exists(&self, p: &Path) -> Result<bool> {
        Ok(p.exists())
    }
    fn children(&self, p: &Path) -> Result<Vec<PathBuf>> {
        let dir_reader = fs::read_dir(p).map_err(|e| map_err_with_name("children", p, e))?;
        let filenames = dir_reader
            .map(|r| {
                if !r.is_ok() {
                    Path::new("").to_owned()
                } else {
                    let direntry = r.unwrap();
                    Path::new(&direntry.file_name()).to_owned()
                }
            })
            .filter(|s| !s.as_os_str().is_empty());
        Ok(Vec::from_iter(filenames))
    }

    fn size_of(&self, p: &Path) -> Result<usize> {
        let mut f = sgx_tprotected_fs::OpenOptions::default()
            .read(true)
            .open_with_key(p, self.key)
            .map_err(|e| map_err_with_name("size_of (open)", p, e))?;
        let size = f.seek(SeekFrom::End(0))?;
        Ok(size as usize)
    }

    fn delete(&self, p: &Path) -> Result<()> {
        Ok(fs::remove_file(p).map_err(|e| map_err_with_name("delete", p, e))?)
    }
    fn mkdir(&self, p: &Path) -> Result<()> {
        Ok(fs::create_dir_all(p).map_err(|e| map_err_with_name("mkdir", p, e))?)
    }
    fn rmdir(&self, p: &Path) -> Result<()> {
        Ok(fs::remove_dir_all(p).map_err(|e| map_err_with_name("rmdir", p, e))?)
    }
    fn rename(&self, old: &Path, new: &Path) -> Result<()> {
        let old_name = old
            .file_name()
            .map(|f| f.to_str())
            .flatten()
            .ok_or(map_err_with_name(
                "rename1",
                old,
                io::Error::from_raw_os_error(21),
            ))?;
        let new_name = new
            .file_name()
            .map(|f| f.to_str())
            .flatten()
            .ok_or(map_err_with_name(
                "rename2",
                old,
                io::Error::from_raw_os_error(21),
            ))?;

        {
            let mut f = sgx_tprotected_fs::OpenOptions::default()
                .append(true)
                .open_with_key(old, self.key)
                .map_err(|e| map_err_with_name("rename (open)", old, e))?;
            f.rename(old_name, new_name)?;
        }

        Ok(fs::rename(old, new).map_err(|e| map_err_with_name("rename", old, e))?)
    }

    fn lock(&self, p: &Path) -> Result<FileLock> {
        let mut locks = self.locks.lock().unwrap();

        if locks.contains_key(&p.to_str().unwrap().to_string()) {
            Err(Status::new(StatusCode::AlreadyExists, "Lock is held"))
        } else {
            let f = sgx_tprotected_fs::OpenOptions::default()
                .write(true)
                .append(false)
                .open_with_key(p, self.key)
                .map_err(|e| map_err_with_name("lock_sgx: ", p, e))?;

            locks.insert(p.to_str().unwrap().to_string(), f);
            let lock = FileLock {
                id: p.to_str().unwrap().to_string(),
            };
            Ok(lock)
        }
    }
    fn unlock(&self, l: FileLock) -> Result<()> {
        let mut locks = self.locks.lock().unwrap();
        if !locks.contains_key(&l.id) {
            return err(
                StatusCode::LockError,
                &format!("unlocking a file that is not locked: {}", l.id),
            );
        } else {
            locks.remove(&l.id).unwrap();
            Ok(())
        }
    }

    fn new_logger(&self, p: &Path) -> Result<Logger> {
        self.open_appendable_file(p)
            .map(|dst| Logger::new(Box::new(dst)))
    }

    fn micros(&self) -> u64 {
        micros()
    }
}

#[cfg(feature = "enclave_unit_test")]
pub mod tests {
    use super::*;
    use std::convert::AsRef;
    use std::io::Write;
    use std::iter::FromIterator;
    use teaclave_test_utils::*;

    pub fn run_tests() -> bool {
        run_tests!(test_files, test_locking, test_dirs,)
    }

    fn test_files() {
        let n = "testfile.xyz".to_string();
        let name = n.as_ref();
        let env = PosixDiskEnv::new_with([0u8; 16]);

        // exists, size_of, delete
        assert!(env.open_appendable_file(name).is_ok());
        assert!(env.exists(name).unwrap_or(false));
        assert_eq!(env.size_of(name).unwrap_or(1), 0);
        assert!(env.delete(name).is_ok());

        assert!(env.open_writable_file(name).is_ok());
        assert!(env.exists(name).unwrap_or(false));
        assert_eq!(env.size_of(name).unwrap_or(1), 0);
        assert!(env.delete(name).is_ok());

        {
            {
                // write
                let mut f = env.open_writable_file(name).unwrap();
                let _ = f.write("123xyz".as_bytes());
            }
            assert_eq!(6, env.size_of(name).unwrap_or(0));

            // rename
            let newname = Path::new("testfile2.xyz");
            assert!(env.rename(name, newname).is_ok());
            assert_eq!(false, env.size_of(newname).is_err());
            assert!(!env.exists(name).unwrap());
            // rename back so that the remaining tests can use the file.
            assert!(env.rename(newname, name).is_ok());
        }

        assert!(env.open_sequential_file(name).is_ok());
        assert!(env.open_random_access_file(name).is_ok());

        assert!(env.delete(name).is_ok());
    }

    fn test_locking() {
        let env = PosixDiskEnv::new_with([0u8; 16]);
        let n = "acquire_lock.123".to_string();
        let name = n.as_ref();

        {
            {
                let mut f = env.open_writable_file(name).unwrap();
                let _ = f.write("123xyz".as_bytes());
            }
            assert_eq!(env.size_of(name).unwrap_or(0), 6);
        }

        {
            let r = env.lock(name);
            assert!(r.is_ok());
            env.unlock(r.unwrap()).unwrap();
        }

        {
            let r = env.lock(name);
            assert!(r.is_ok());
            let s = env.lock(name);
            assert!(s.is_err());
            env.unlock(r.unwrap()).unwrap();
        }

        assert!(env.delete(name).is_ok());
    }

    fn test_dirs() {
        let d = "subdir/";
        let dirname = d.as_ref();
        let env = PosixDiskEnv::new_with([0u8; 16]);

        assert!(env.mkdir(dirname).is_ok());
        assert!(env
            .open_writable_file(
                String::from_iter(vec![d.to_string(), "f1.txt".to_string()].into_iter()).as_ref()
            )
            .is_ok());
        assert_eq!(env.children(dirname).unwrap().len(), 1);
        assert!(env.rmdir(dirname).is_ok());
    }
}
