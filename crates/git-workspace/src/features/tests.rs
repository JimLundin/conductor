use super::{Attribute, Setting, accepts, configuration};

#[test]
fn installed_filters_only_matter_on_paths_that_select_them() {
    assert_eq!(configuration("filter.lfs.required"), Setting::Any);
    assert!(accepts("filter", Attribute::Unspecified));
    assert!(!accepts("filter", Attribute::Value("lfs")));
    assert!(accepts("merge", Attribute::Unset));
    assert!(!accepts("merge", Attribute::Value("custom")));
}

#[test]
fn unsupported_index_and_object_modes_refuse_before_capture() {
    for name in [
        "core.sparsecheckout",
        "core.splitindex",
        "index.sparse",
        "remote.origin.promisor",
    ] {
        assert_eq!(configuration(name), Setting::Disabled);
    }
    assert_eq!(
        configuration("extensions.partialclone"),
        Setting::Unsupported
    );
    assert_eq!(configuration("remote.origin.url"), Setting::Any);
}
