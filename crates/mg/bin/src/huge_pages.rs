use std::io;

/// Turn off transparent huge pages for this process and everything it starts.
///
/// Where a host sets `transparent_hugepage=always`, freed heap pages stay
/// resident inside 2 MiB pages and khugepaged fills sparse ranges with zeroes.
/// A seat `fedimintd` then holds several times its anonymous memory, so the
/// same seat costs a different amount on each host
/// ([REQ-seat-capacity-default](../../specs/REQ-seat-capacity-default.md)).
/// The setting survives `fork` and `execve`, so seats inherit it.
pub fn disable() -> io::Result<()> {
    // SAFETY: PR_SET_THP_DISABLE reads only its integer arguments.
    if unsafe { libc::prctl(libc::PR_SET_THP_DISABLE, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn disabled() -> bool {
        // SAFETY: PR_GET_THP_DISABLE takes no pointers and only returns a value.
        match unsafe { libc::prctl(libc::PR_GET_THP_DISABLE, 0, 0, 0, 0) } {
            0 => false,
            1 => true,
            other => panic!("PR_GET_THP_DISABLE returned {other}"),
        }
    }

    fn child(mode: &str) {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "huge_pages::tests::disables_huge_pages_and_children_inherit_it",
                "--nocapture",
            ])
            .env("MG_TEST_HUGE_PAGES", mode)
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
    fn disables_huge_pages_and_children_inherit_it() {
        match std::env::var("MG_TEST_HUGE_PAGES").as_deref() {
            Ok("disable") => {
                // Only in a child: the setting cannot leak into parallel tests.
                assert!(
                    !disabled(),
                    "the test process must start with huge pages on"
                );
                disable().unwrap();
                assert!(disabled());
                child("inherit");
            }
            Ok("inherit") => assert!(disabled(), "a seat process must inherit the setting"),
            _ => child("disable"),
        }
    }
}
