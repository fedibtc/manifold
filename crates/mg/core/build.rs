//! `FEDIMINT_BUILD_CODE_VERSION`: the bundled `fedimintd`'s build hash.

fn main() {
    fedimint_build::set_code_version();
}
