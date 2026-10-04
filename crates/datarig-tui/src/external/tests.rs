use super::*;
use std::collections::HashMap;

fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |k| map.get(k).cloned()
}

#[test]
fn visual_comes_before_editor_and_vi_comes_last() {
    let cmd = editor_command(env(&[("VISUAL", "nvim -u NONE"), ("EDITOR", "nano")])).unwrap();
    assert_eq!(
        cmd,
        EditorCommand { var: Some("VISUAL"), program: "nvim".into(), args: vec!["-u".into(), "NONE".into()] }
    );
    let cmd = editor_command(env(&[("EDITOR", "code -w")])).unwrap();
    assert_eq!(cmd, EditorCommand { var: Some("EDITOR"), program: "code".into(), args: vec!["-w".into()] });
    let cmd = editor_command(env(&[])).unwrap();
    assert_eq!(cmd, EditorCommand { var: None, program: DEFAULT_EDITOR.into(), args: vec![] });
}

/// Words split as a shell splits them (quotes), and nothing else of a shell applies.
#[test]
fn the_command_splits_into_words_without_a_shell() {
    let cmd = editor_command(env(&[("VISUAL", r#"'/opt/my editor/bin/ed' --title "a b" $HOME"#)])).unwrap();
    assert_eq!(cmd.program, "/opt/my editor/bin/ed");
    assert_eq!(cmd.args, ["--title", "a b", "$HOME"]);
}

#[test]
fn a_blank_variable_counts_as_unset() {
    let cmd = editor_command(env(&[("VISUAL", "  "), ("EDITOR", "nano")])).unwrap();
    assert_eq!((cmd.var, cmd.program.as_str()), (Some("EDITOR"), "nano"));
    let cmd = editor_command(env(&[("VISUAL", ""), ("EDITOR", "")])).unwrap();
    assert_eq!(cmd.var, None);
}

/// Unknown is not absent: a `$VISUAL` that does not split is said, not skipped for `$EDITOR`.
#[test]
fn a_variable_that_does_not_split_is_an_error() {
    let err = editor_command(env(&[("VISUAL", "vim 'unclosed"), ("EDITOR", "nano")])).unwrap_err();
    assert!(matches!(err, EditFailure::Command { var: "VISUAL", .. }), "{err:?}");
}

#[test]
fn what_the_editor_saved_comes_back_without_the_last_line_break() {
    let w = "SELECT 1\n".as_bytes();
    assert_eq!(read_back("SELECT 1", w, b"SELECT 1\n".to_vec()), Edited::Unchanged);
    assert_eq!(read_back("SELECT 1", w, b"SELECT 1".to_vec()), Edited::Unchanged, "no line break at the end");
    assert_eq!(read_back("SELECT 1", w, b"SELECT 2\n".to_vec()), Edited::Changed("SELECT 2".into()));
    assert_eq!(read_back("SELECT 1", w, b"a\r\nb\r\n".to_vec()), Edited::Changed("a\nb".into()));
    assert_eq!(read_back("SELECT 1", w, b"a\n\n".to_vec()), Edited::Changed("a\n".into()), "only one line break goes");
    assert_eq!(read_back("SELECT 1", w, Vec::new()), Edited::Changed(String::new()), "a file emptied on purpose");
    assert_eq!(read_back("SELECT 1", w, vec![0xff, b'\n']), Edited::Failed(EditFailure::NotUtf8));
    assert_eq!(read_back("", b"\n", b"\n".to_vec()), Edited::Unchanged);
}

#[test]
fn without_a_state_directory_nothing_runs() {
    let mut term = FakeTerm::default();
    let cmd = EditorCommand { var: None, program: "false".into(), args: vec![] };
    assert_eq!(edit(None, "x", &cmd, &mut term).unwrap(), Edited::Failed(EditFailure::NoStateDir));
    assert_eq!(term.calls, Vec::<&str>::new());
}

/// Records what the terminal was asked, and what the editor's directory held when the
/// terminal was given away.
#[derive(Default)]
struct FakeTerm {
    calls: Vec<&'static str>,
    watch: Option<PathBuf>,
    seen: Vec<(PathBuf, Vec<u8>, u32)>,
    fail_reclaim: bool,
}

impl Handover for FakeTerm {
    fn release(&mut self) -> io::Result<()> {
        self.calls.push("release");
        if let Some(dir) = &self.watch {
            for e in fs::read_dir(dir)? {
                let p = e?.path();
                #[cfg(unix)]
                let mode = std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&p)?.permissions()) & 0o777;
                #[cfg(not(unix))]
                let mode = 0;
                self.seen.push((p.clone(), fs::read(&p)?, mode));
            }
        }
        Ok(())
    }

    fn reclaim(&mut self) -> io::Result<()> {
        self.calls.push("reclaim");
        if self.fail_reclaim { Err(io::Error::other("gone")) } else { Ok(()) }
    }
}

#[cfg(unix)]
mod scripts {
    use super::*;

    /// A scratch directory for one test: the state directory's `edit` folder and the scripts.
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("datarig-external-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// `sh <script>`: the editor, a script with `body` (`$1` is the file).
    fn script(root: &Path, body: &str) -> EditorCommand {
        let path = root.join("editor.sh");
        fs::write(&path, body).unwrap();
        EditorCommand { var: Some("VISUAL"), program: "sh".into(), args: vec![path.to_string_lossy().into_owned()] }
    }

    fn run(root: &Path, text: &str, body: &str) -> (Edited, FakeTerm) {
        let dir = root.join("state").join("edit");
        let mut term = FakeTerm { watch: Some(dir.clone()), ..Default::default() };
        let out = edit(Some(&dir), text, &script(root, body), &mut term).unwrap();
        (out, term)
    }

    #[test]
    fn a_saved_change_comes_back_and_the_file_is_private_and_removed() {
        let root = scratch("change");
        let (out, term) = run(&root, "SELECT 1", "printf 'SELECT 2\\n' > \"$1\"\n");
        assert_eq!(out, Edited::Changed("SELECT 2".into()));
        assert_eq!(term.calls, ["release", "reclaim"]);
        let [(path, bytes, mode)] = term.seen.as_slice() else { panic!("{:?}", term.seen) };
        assert_eq!(bytes, b"SELECT 1\n", "the text and a line break, as an editor saves a file");
        assert_eq!(*mode, 0o600);
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("sql"));
        assert!(!path.exists(), "removed afterwards");
        let dir = root.join("state").join("edit");
        let mode = std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&dir).unwrap().permissions()) & 0o777;
        assert_eq!(mode, 0o700);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn quitting_without_saving_is_no_change() {
        let root = scratch("quit");
        let (out, term) = run(&root, "SELECT 1;\n\nSELECT 2;", "exit 0\n");
        assert_eq!(out, Edited::Unchanged);
        assert!(!term.seen[0].0.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    /// Vim's `:cq`: the editor says it failed, so what it may have saved is not taken.
    #[test]
    fn a_failing_editor_keeps_the_text_and_says_how_it_ended() {
        let root = scratch("fail");
        let (out, term) = run(&root, "SELECT 1", "printf 'DROP TABLE t' > \"$1\"\nexit 3\n");
        assert_eq!(out, Edited::Failed(EditFailure::Exit { program: "sh".into(), how: Ended::Code(3) }));
        assert_eq!(term.calls, ["release", "reclaim"]);
        assert!(!term.seen[0].0.exists());
        let (out, _) = run(&root, "SELECT 1", "kill -9 $$\n");
        assert_eq!(out, Edited::Failed(EditFailure::Exit { program: "sh".into(), how: Ended::Signal(9) }));
        fs::remove_dir_all(&root).unwrap();
    }

    /// Unknown is not absent: a file that cannot be read back is not an empty text.
    #[test]
    fn a_file_that_is_gone_is_not_an_empty_text() {
        let root = scratch("gone");
        let (out, _) = run(&root, "SELECT 1", "rm \"$1\"\n");
        assert!(matches!(out, Edited::Failed(EditFailure::Read(_))), "{out:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_arguments_come_before_the_file() {
        let root = scratch("args");
        let log = root.join("args.log");
        let body = format!("printf '%s|' \"$@\" > '{}'\n", log.display());
        let mut cmd = script(&root, &body);
        cmd.args.extend(["-w".to_string(), "two words".to_string()]);
        let dir = root.join("state").join("edit");
        let out = edit(Some(&dir), "x", &cmd, &mut FakeTerm::default()).unwrap();
        assert_eq!(out, Edited::Unchanged);
        let args = fs::read_to_string(&log).unwrap();
        let parts: Vec<&str> = args.trim_end_matches('|').split('|').collect();
        assert_eq!(parts[..2], ["-w", "two words"]);
        assert!(parts[2].starts_with(&dir.to_string_lossy().into_owned()) && parts[2].ends_with(".sql"), "{args}");
        fs::remove_dir_all(&root).unwrap();
    }

    /// An editor that does not start: said, and the terminal was taken back.
    #[test]
    fn an_editor_that_does_not_start_is_said() {
        let root = scratch("missing");
        let dir = root.join("state").join("edit");
        let cmd = EditorCommand {
            var: Some("EDITOR"),
            program: root.join("no-such-editor").to_string_lossy().into(),
            args: vec![],
        };
        let mut term = FakeTerm { watch: Some(dir.clone()), ..Default::default() };
        let out = edit(Some(&dir), "x", &cmd, &mut term).unwrap();
        assert!(matches!(out, Edited::Failed(EditFailure::Spawn { .. })), "{out:?}");
        assert_eq!(term.calls, ["release", "reclaim"]);
        assert!(!term.seen[0].0.exists(), "removed");
        fs::remove_dir_all(&root).unwrap();
    }

    /// A state directory that is not a directory: said, nothing runs.
    #[test]
    fn a_file_that_cannot_be_made_is_said_before_anything_runs() {
        let root = scratch("blocked");
        fs::write(root.join("state"), "a file").unwrap();
        let dir = root.join("state").join("edit");
        let mut term = FakeTerm::default();
        let out = edit(Some(&dir), "x", &script(&root, "exit 0\n"), &mut term).unwrap();
        assert!(matches!(out, Edited::Failed(EditFailure::File(_))), "{out:?}");
        assert!(term.calls.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    /// A terminal that cannot be taken back ends the program (an error); the file goes anyway.
    #[test]
    fn a_terminal_that_cannot_be_taken_back_is_an_error() {
        let root = scratch("reclaim");
        let dir = root.join("state").join("edit");
        let mut term = FakeTerm { watch: Some(dir.clone()), fail_reclaim: true, ..Default::default() };
        assert!(edit(Some(&dir), "x", &script(&root, "exit 0\n"), &mut term).is_err());
        assert!(!term.seen[0].0.exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
