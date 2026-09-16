use super::validate_external_dir_for_source;
use crate::recovery::workflow_test_support::TestDir;

#[test]
fn installed_approval_store_does_not_require_original_build_source() {
    let dir = TestDir::new("approval-deployed-path");
    let absent_build_source = dir.path.join("original-source-is-not-installed");
    let data = dir.path.join("runtime-data");
    assert!(validate_external_dir_for_source(&data, &absent_build_source).is_ok());
    assert!(validate_external_dir_for_source(&data, &dir.path).is_err());
    assert!(
        !data.exists(),
        "validation must not create a data directory"
    );
}
