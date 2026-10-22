use std::io;

fn current() -> io::Result<libc::rlimit> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: limit is writable storage for one rlimit.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(limit)
}

/// Use the host's allowed headroom so DNS bursts do not hit a low soft limit.
/// This does not reduce the DNS traffic itself or change the host's hard limit.
pub fn raise() -> io::Result<()> {
    let mut limit = current()?;
    if limit.rlim_cur == limit.rlim_max {
        return Ok(());
    }
    limit.rlim_cur = limit.rlim_max;
    // SAFETY: limit is an initialized rlimit; the hard limit stays unchanged.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn child(mode: &str) {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "file_limit::tests::raises_soft_limit_and_children_inherit_it",
                "--nocapture",
            ])
            .env("MG_TEST_FILE_LIMIT", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn raises_soft_limit_and_children_inherit_it() {
        match std::env::var("MG_TEST_FILE_LIMIT").as_deref() {
            Ok("raise") => {
                // Lower limits only in a child so parallel tests are unaffected.
                let limit = libc::rlimit {
                    rlim_cur: 64,
                    rlim_max: 256,
                };
                // SAFETY: limit is initialized; this subprocess owns its limits.
                assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
                raise().unwrap();
                let limit = current().unwrap();
                assert_eq!((limit.rlim_cur, limit.rlim_max), (256, 256));
                raise().unwrap();
                child("inherit");
            }
            Ok("inherit") => {
                let limit = current().unwrap();
                assert_eq!((limit.rlim_cur, limit.rlim_max), (256, 256));
            }
            _ => child("raise"),
        }
    }
}
