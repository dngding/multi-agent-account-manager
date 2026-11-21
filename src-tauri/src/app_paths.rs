use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

/// An invalid explicit directory must never fall back to the default account store.
pub fn resolve_data_dir(default: PathBuf, override_dir: Option<OsString>) -> io::Result<PathBuf> {
  match override_dir {
    None => Ok(default),
    Some(raw) => {
      let path = PathBuf::from(raw);
      if !path.is_absolute() {
        return Err(io::Error::new(
          io::ErrorKind::InvalidInput,
          "DIEHAO_DATA_DIR must be a non-empty absolute directory path",
        ));
      }
      Ok(path)
    }
  }
}

#[cfg(test)]
mod tests {
  use super::resolve_data_dir;
  use std::ffi::OsString;
  use std::path::PathBuf;

  #[test]
  fn explicit_data_directory_does_not_use_default_store() {
    let default = std::env::temp_dir().join("diehao-default-store");
    let isolated = std::env::temp_dir().join("diehao-isolated-store");
    assert_eq!(resolve_data_dir(default.clone(), None).unwrap(), default);
    assert_eq!(
      resolve_data_dir(default, Some(isolated.clone().into_os_string())).unwrap(),
      isolated
    );
  }

  #[test]
  fn invalid_override_fails_instead_of_falling_back() {
    for invalid in ["", "relative/profile", "."] {
      assert!(resolve_data_dir(PathBuf::from("default"), Some(OsString::from(invalid))).is_err());
    }
    #[cfg(windows)]
    for invalid in [r"C:relative", r"\rooted-without-drive"] {
      assert!(resolve_data_dir(PathBuf::from("default"), Some(OsString::from(invalid))).is_err());
    }
  }
}
