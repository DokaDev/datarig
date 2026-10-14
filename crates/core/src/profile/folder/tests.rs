use super::*;

fn p(s: &str) -> FolderPath {
    FolderPath::parse(s).unwrap()
}

#[test]
fn path_rules() {
    assert_eq!(p("work/prod").as_str(), "work/prod");
    assert_eq!(p("本番 サーバ/db 1").name(), "db 1", "inner spaces are fine");
    assert_eq!(FolderPath::parse(""), Err(FolderError::Empty));
    for bad in ["/a", "a/", "a//b"] {
        assert_eq!(FolderPath::parse(bad), Err(FolderError::EmptySegment), "{bad}");
    }
    for bad in [" a", "a /b", "a/b "] {
        assert_eq!(FolderPath::parse(bad), Err(FolderError::Spaces), "{bad}");
    }
    for bad in [".", "a/..", "../a"] {
        assert_eq!(FolderPath::parse(bad), Err(FolderError::Dots), "{bad}");
    }
    let x = p("a/b/c");
    assert_eq!((x.depth(), x.parent(), x.name()), (2, Some(p("a/b")), "c"));
    assert_eq!(x.with_ancestors(), [p("a"), p("a/b"), p("a/b/c")]);
    assert!(x.is_within(&p("a")) && x.is_within(&x) && !p("ab").is_within(&p("a")));
    assert_eq!(x.rebase(&p("a/b"), &p("z")), Some(p("z/c")));
    assert_eq!(p("ab").rebase(&p("a"), &p("z")), None);
}

#[test]
fn tree_children_rename_and_expanded_state() {
    let mut f = Folders::default();
    f.insert(&p("work/prod"));
    f.insert(&p("local"));
    f.insert(&p("Work2"));
    assert!(f.contains(&p("work")), "ancestors are added");
    let names = |v: Vec<&FolderPath>| v.into_iter().map(|p| p.as_str().to_string()).collect::<Vec<_>>();
    assert_eq!(names(f.children(None)), ["local", "work", "Work2"], "by name, ignoring case");
    assert_eq!(names(f.children(Some(&p("work")))), ["work/prod"]);
    assert!(f.toggle(&p("work")) && f.toggle(&p("work/prod")));
    assert!(!f.toggle(&p("nope")), "unknown folders are not expanded");
    f.rename(&p("work"), &p("office/work"));
    assert!(f.contains(&p("office")) && f.contains(&p("office/work/prod")) && !f.contains(&p("work")));
    assert!(f.is_expanded(&p("office/work")) && f.is_expanded(&p("office/work/prod")), "state follows");
    assert!(!f.toggle(&p("office/work")), "collapses again");
    f.set_expanded([&p("local"), &p("gone")]);
    assert_eq!(f.expanded().collect::<Vec<_>>(), [&p("local")]);
    f.remove(&p("office"));
    assert_eq!(names(f.iter().collect()), ["Work2", "local"]);
}
