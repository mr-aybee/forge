#![allow(clippy::unwrap_used)]

use base64::Engine as _;
use forge_core::files::{FileOp, RootSpec};
use serde_json::Value;

use super::{Ops, Root};

struct Env {
    base: tempfile::TempDir,
    store: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self { base: tempfile::tempdir().unwrap(), store: tempfile::tempdir().unwrap() }
    }

    fn p(&self, rel: &str) -> std::path::PathBuf {
        self.base.path().join(rel)
    }

    fn run(&self, json: &str) -> Result<Value, String> {
        let json = json.replace("ROOT", r#"{"kind":"node"}"#);
        let op: FileOp = serde_json::from_str(&json).unwrap_or_else(|e| panic!("{json}: {e}"));
        op.validate()?;
        let root = Root::at(self.base.path(), self.store.path())?;

        Ops::new(&root, &RootSpec::Node, false).run(&op)
    }

    fn upload(&self, path: &str, data: &[u8], overwrite: &str) -> Result<Value, String> {
        let id = "u1";
        let b64 = base64::engine::general_purpose::STANDARD.encode(data);
        self.run(&format!(
            r#"{{"op":"upload_chunk","root":ROOT,"path":"{path}","upload":"{id}","offset":0,"data_b64":"{b64}"}}"#
        ))?;
        let sha = super::hex(ring::digest::digest(&ring::digest::SHA256, data).as_ref());

        self.run(&format!(
            r#"{{"op":"upload_commit","root":ROOT,"path":"{path}","upload":"{id}","size":{},"sha256":"{sha}","overwrite":{overwrite}}}"#,
            data.len()
        ))
    }
}

fn names(v: &Value) -> Vec<String> {
    v["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_owned()).collect()
}

fn text(v: &Value) -> String {
    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(v["content_b64"].as_str().unwrap()).unwrap())
        .unwrap()
}

#[test]
fn lists_pages_and_reads_with_versions() {
    let e = Env::new();
    std::fs::create_dir(e.p("app")).unwrap();
    std::fs::write(e.p("app/config.json"), b"{\"a\":1}").unwrap();
    std::fs::write(e.p("app/b.txt"), b"hello world").unwrap();
    std::fs::write(e.p("app/.env"), b"X=1").unwrap();

    let l = e.run(r#"{"op":"list","root":ROOT,"path":"/app"}"#).unwrap();
    assert_eq!(names(&l), ["b.txt", "config.json"]);
    let l = e.run(r#"{"op":"list","root":ROOT,"path":"/app","hidden":true,"offset":1,"limit":1}"#).unwrap();
    assert_eq!(l["total"], 3);
    assert_eq!(names(&l), ["b.txt"]);

    let r = e.run(r#"{"op":"read","root":ROOT,"path":"/app/b.txt","offset":6,"length":100}"#).unwrap();
    assert_eq!(text(&r), "world");
    assert_eq!(r["eof"], true);
    let v = r["version"].as_str().unwrap().to_owned();
    let same = format!(r#"{{"op":"read","root":ROOT,"path":"/app/b.txt","length":5,"version":"{v}"}}"#);
    assert!(e.run(&same).is_ok());
    std::fs::write(e.p("app/b.txt"), b"changed content").unwrap();
    assert!(e.run(&same).unwrap_err().contains("source changed"));

    assert!(e.run(r#"{"op":"read","root":ROOT,"path":"/app","length":5}"#).is_err());
    assert!(e.run(r#"{"op":"list","root":ROOT,"path":"/missing"}"#).unwrap_err().contains("not found"));
    let s = e.run(r#"{"op":"stat","root":ROOT,"path":"/app/config.json"}"#).unwrap();
    assert_eq!(s["entry"]["kind"], "file");
    assert_eq!(s["entry"]["size"], 7);
    let h = e.run(r#"{"op":"hash","root":ROOT,"path":"/app/config.json"}"#).unwrap();
    assert_eq!(h["sha256"].as_str().unwrap().len(), 64);
}

#[test]
fn uploads_commit_atomically_with_conflict_checks() {
    let e = Env::new();
    std::fs::create_dir(e.p("d")).unwrap();

    // New file.
    let r = e.upload("/d/new.txt", b"first", r#"{"mode":"never"}"#).unwrap();
    assert_eq!(std::fs::read(e.p("d/new.txt")).unwrap(), b"first");
    let v1 = r["version"].as_str().unwrap().to_owned();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(e.p("d/new.txt")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "a new file must not keep the partial file's 0600");
    }
    // Never overwrites.
    assert!(e.upload("/d/new.txt", b"x", r#"{"mode":"never"}"#).unwrap_err().contains("already exists"));

    // Matching version replaces and keeps a revision.
    e.upload("/d/new.txt", b"second", &format!(r#"{{"mode":"if_version","version":"{v1}"}}"#)).unwrap();
    assert_eq!(std::fs::read(e.p("d/new.txt")).unwrap(), b"second");

    // A stale version is a conflict and leaves the file alone.
    let err = e.upload("/d/new.txt", b"third", &format!(r#"{{"mode":"if_version","version":"{v1}"}}"#)).unwrap_err();
    assert!(err.contains("source changed"), "{err}");
    assert_eq!(std::fs::read(e.p("d/new.txt")).unwrap(), b"second");

    // Partial files never show up in listings.
    let l = e.run(r#"{"op":"list","root":ROOT,"path":"/d","hidden":true}"#).unwrap();
    assert_eq!(names(&l), ["new.txt"]);

    // Revisions: the "first" content was saved before it was replaced.
    let revs = e.run(r#"{"op":"revisions","root":ROOT,"path":"/d/new.txt"}"#).unwrap();
    let id = revs["revisions"][0]["id"].as_str().unwrap().to_owned();
    let old = e
        .run(&format!(r#"{{"op":"revision_read","root":ROOT,"path":"/d/new.txt","revision":"{id}","length":100}}"#))
        .unwrap();
    assert_eq!(text(&old), "first");

    // Checksum and size are verified before anything moves.
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"abc");
    e.run(&format!(
        r#"{{"op":"upload_chunk","root":ROOT,"path":"/d/c.txt","upload":"u9","offset":0,"data_b64":"{b64}"}}"#
    ))
    .unwrap();
    let bad = format!(
        r#"{{"op":"upload_commit","root":ROOT,"path":"/d/c.txt","upload":"u9","size":3,"sha256":"{}","overwrite":{{"mode":"never"}}}}"#,
        "0".repeat(64)
    );
    assert!(e.run(&bad).unwrap_err().contains("checksum"));
    assert!(!e.p("d/c.txt").exists());

    // Resume: status reports the bytes received; a wrong offset is refused.
    let st = e.run(r#"{"op":"upload_status","root":ROOT,"path":"/d/c.txt","upload":"u9"}"#).unwrap();
    assert_eq!(st["received"], 3);
    let more = base64::engine::general_purpose::STANDARD.encode(b"d");
    let wrong = format!(
        r#"{{"op":"upload_chunk","root":ROOT,"path":"/d/c.txt","upload":"u9","offset":1,"data_b64":"{more}"}}"#
    );
    assert!(e.run(&wrong).unwrap_err().contains("resume"));
    e.run(r#"{"op":"upload_abort","root":ROOT,"path":"/d/c.txt","upload":"u9"}"#).unwrap();
    let st = e.run(r#"{"op":"upload_status","root":ROOT,"path":"/d/c.txt","upload":"u9"}"#).unwrap();
    assert_eq!(st["received"], 0);
}

#[test]
fn mkdir_rename_copy_delete_trash_restore() {
    let e = Env::new();
    e.run(r#"{"op":"mkdir","root":ROOT,"path":"/a"}"#).unwrap();
    assert!(e.run(r#"{"op":"mkdir","root":ROOT,"path":"/a"}"#).unwrap_err().contains("exists"));
    std::fs::write(e.p("a/f.txt"), b"data").unwrap();
    std::fs::write(e.p("g.txt"), b"other").unwrap();

    // Copy a tree; no overwrite by default.
    e.run(r#"{"op":"copy","root":ROOT,"from":"/a","to":"/b"}"#).unwrap();
    assert_eq!(std::fs::read(e.p("b/f.txt")).unwrap(), b"data");
    assert!(e.run(r#"{"op":"copy","root":ROOT,"from":"/g.txt","to":"/a/f.txt"}"#).is_err());
    e.run(r#"{"op":"copy","root":ROOT,"from":"/g.txt","to":"/a/f.txt","overwrite":true}"#).unwrap();
    assert_eq!(std::fs::read(e.p("a/f.txt")).unwrap(), b"other");

    // Rename refuses to replace unless asked.
    assert!(e.run(r#"{"op":"rename","root":ROOT,"from":"/g.txt","to":"/b/f.txt"}"#).unwrap_err().contains("exists"));
    e.run(r#"{"op":"rename","root":ROOT,"from":"/g.txt","to":"/h.txt"}"#).unwrap();
    assert!(e.p("h.txt").exists() && !e.p("g.txt").exists());

    // Delete goes to the trash and can be restored.
    let d = e.run(r#"{"op":"delete","root":ROOT,"path":"/b"}"#).unwrap();
    assert!(!e.p("b").exists());
    let t = e.run(r#"{"op":"trash_list","root":ROOT}"#).unwrap();
    assert_eq!(t["entries"][0]["path"], "/b");
    let id = d["trash_id"].as_str().unwrap();
    e.run(&format!(r#"{{"op":"trash_restore","root":ROOT,"id":"{id}"}}"#)).unwrap();
    assert_eq!(std::fs::read(e.p("b/f.txt")).unwrap(), b"data");
    assert_eq!(e.run(r#"{"op":"trash_list","root":ROOT}"#).unwrap()["entries"].as_array().unwrap().len(), 0);

    // Another root never sees this root's trash.
    e.run(r#"{"op":"delete","root":ROOT,"path":"/h.txt"}"#).unwrap();
    let other = Root::at(e.base.path(), e.store.path()).unwrap();
    let spec = RootSpec::Dir { base: forge_core::files::FilePath::parse("/elsewhere").unwrap() };
    let op: FileOp = serde_json::from_str(r#"{"op":"trash_list","root":{"kind":"dir","base":"/elsewhere"}}"#).unwrap();
    assert_eq!(Ops::new(&other, &spec, false).run(&op).unwrap()["entries"].as_array().unwrap().len(), 0);
    let purged = e.run(r#"{"op":"trash_purge","root":ROOT}"#).unwrap();
    assert_eq!(purged["purged"], 1);

    // Permanent delete.
    e.run(r#"{"op":"delete","root":ROOT,"path":"/a","permanent":true}"#).unwrap();
    assert!(!e.p("a").exists());

    // The root itself can never be changed.
    assert!(e.run(r#"{"op":"delete","root":ROOT,"path":"/","permanent":true}"#).is_err());
}

#[test]
fn search_names_and_content_with_limits() {
    let e = Env::new();
    std::fs::create_dir_all(e.p("etc/nginx/sites")).unwrap();
    std::fs::create_dir_all(e.p("node_modules/x")).unwrap();
    std::fs::write(e.p("etc/nginx/nginx.conf"), b"user www;\nlisten 80;\nserver_name a;\n").unwrap();
    std::fs::write(e.p("etc/nginx/sites/b.conf"), b"listen 443;\n").unwrap();
    std::fs::write(e.p("node_modules/x/listen.conf"), b"listen 1;\n").unwrap();
    std::fs::write(e.p("bin.dat"), b"listen\0\0\0").unwrap();

    let r = e.run(r#"{"op":"search","root":ROOT,"path":"/","spec":{"name":"*.conf"}}"#).unwrap();
    assert_eq!(r["status"], "complete");
    assert_eq!(r["files"].as_array().unwrap().len(), 3);

    let r = e
        .run(r#"{"op":"search","root":ROOT,"path":"/","spec":{"content":"LISTEN","exclude":["node_modules/**"],"context":1}}"#)
        .unwrap();
    let files: Vec<&str> = r["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(files, ["etc/nginx/nginx.conf", "etc/nginx/sites/b.conf"]);
    assert_eq!(r["files"][0]["matches"][0]["line"], 2);
    assert_eq!(r["files"][0]["matches"][0]["before"][0], "user www;");

    let r = e.run(r#"{"op":"search","root":ROOT,"path":"/","spec":{"content":"listen \\d+","regex":true,"case_sensitive":true}}"#).unwrap();
    assert_eq!(r["matches"], 3);
    assert!(
        e.run(r#"{"op":"search","root":ROOT,"path":"/","spec":{"content":"(","regex":true}}"#)
            .unwrap_err()
            .contains("invalid pattern")
    );

    // Resume after a file.
    let r = e
        .run(r#"{"op":"search","root":ROOT,"path":"/","spec":{"name":"*.conf","after":"etc/nginx/nginx.conf"}}"#)
        .unwrap();
    let files: Vec<&str> = r["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(files, ["etc/nginx/sites/b.conf", "node_modules/x/listen.conf"]);
}

#[test]
fn protected_paths_are_refused_everywhere() {
    let e = Env::new();
    #[cfg(unix)]
    let (file, dir) = ("/etc/shadow", "/etc/forge");
    #[cfg(windows)]
    let (file, dir) = ("/C:/Windows/System32/config/SAM", "/C:/ProgramData/Forge");
    for op in [
        format!(r#"{{"op":"read","root":ROOT,"path":"{file}","length":10}}"#),
        format!(r#"{{"op":"mkdir","root":ROOT,"path":"{dir}/x"}}"#),
        format!(r#"{{"op":"copy","root":ROOT,"from":"/x","to":"{dir}/y"}}"#),
        format!(r#"{{"op":"rename","root":ROOT,"from":"{file}","to":"/z"}}"#),
        format!(r#"{{"op":"delete","root":ROOT,"path":"{file}"}}"#),
    ] {
        let err = e.run(&op).unwrap_err();
        assert!(err.starts_with("protected path"), "{op}: {err}");
    }
    // Moving or removing a parent would carry protected content along:
    // refused whole, both as the source and the destination of a move.
    #[cfg(unix)]
    let parents = ["/home/alice", "/home", "/var", "/var/lib", "/root", "/etc", "/etc/ssh"];
    #[cfg(windows)]
    let parents = ["/C:/ProgramData", "/C:/Users/bob", "/C:/Windows/System32", "/C:/PROGRA~3", "/C:"];
    for dir in parents {
        for op in [
            format!(r#"{{"op":"rename","root":ROOT,"from":"{dir}","to":"/stash"}}"#),
            format!(r#"{{"op":"rename","root":ROOT,"from":"/stash","to":"{dir}","overwrite":true}}"#),
            format!(r#"{{"op":"delete","root":ROOT,"path":"{dir}"}}"#),
            format!(r#"{{"op":"delete","root":ROOT,"path":"{dir}","permanent":true}}"#),
        ] {
            let err = e.run(&op).unwrap_err();
            assert!(err.starts_with("protected path"), "{op}: {err}");
        }
    }
    #[cfg(unix)]
    {
        std::fs::create_dir_all(e.p("home/alice/.ssh")).unwrap();
        std::fs::write(e.p("home/alice/.ssh/id_ed25519"), b"key").unwrap();
        std::fs::create_dir_all(e.p("home/alice/projects")).unwrap();
        assert!(e.run(r#"{"op":"delete","root":ROOT,"path":"/home/alice","permanent":true}"#).is_err());
        assert!(e.p("home/alice/.ssh/id_ed25519").exists());
        // Folders that cannot hold protected paths still move.
        e.run(r#"{"op":"rename","root":ROOT,"from":"/home/alice/projects","to":"/home/alice/work"}"#).unwrap();
    }
    #[cfg(unix)]
    {
        std::fs::create_dir_all(e.p("etc/forge")).unwrap();
        std::fs::write(e.p("etc/shadow"), b"x").unwrap();
        std::fs::write(e.p("etc/hosts"), b"x").unwrap();
        let l = e.run(r#"{"op":"list","root":ROOT,"path":"/etc"}"#).unwrap();
        let flag =
            |n: &str| l["entries"].as_array().unwrap().iter().find(|x| x["name"] == n).unwrap()["protected"].clone();
        assert_eq!(flag("shadow"), true);
        assert_eq!(flag("forge"), true);
        assert_eq!(flag("hosts"), false);
        // A copy of a tree that contains a protected path stops there.
        let err = e.run(r#"{"op":"copy","root":ROOT,"from":"/etc","to":"/etc2"}"#).unwrap_err();
        assert!(err.starts_with("protected path"), "{err}");
    }
}

#[cfg(unix)]
#[test]
fn replacing_a_setuid_file_drops_setuid_and_setgid() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    std::fs::write(e.p("tool"), b"original").unwrap();
    std::fs::set_permissions(e.p("tool"), std::fs::Permissions::from_mode(0o6755)).unwrap();
    e.upload("/tool", b"replaced", r#"{"mode":"always"}"#).unwrap();
    assert_eq!(std::fs::read(e.p("tool")).unwrap(), b"replaced");
    let mode = std::fs::metadata(e.p("tool")).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o755, "the other permission bits stay; setuid/setgid go");
}

#[cfg(unix)]
#[test]
fn never_follows_links_out_of_the_root() {
    let e = Env::new();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), b"top secret").unwrap();
    std::fs::create_dir(e.p("d")).unwrap();
    std::os::unix::fs::symlink(outside.path(), e.p("escape")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), e.p("d/file-link")).unwrap();

    let l = e.run(r#"{"op":"list","root":ROOT,"path":"/"}"#).unwrap();
    let esc = l["entries"].as_array().unwrap().iter().find(|x| x["name"] == "escape").unwrap();
    assert_eq!(esc["kind"], "symlink");
    assert_eq!(esc["link_target"], outside.path().display().to_string());

    for op in [
        r#"{"op":"list","root":ROOT,"path":"/escape"}"#,
        r#"{"op":"read","root":ROOT,"path":"/escape/secret","length":10}"#,
        r#"{"op":"read","root":ROOT,"path":"/d/file-link","length":10}"#,
        r#"{"op":"mkdir","root":ROOT,"path":"/escape/new"}"#,
        r#"{"op":"copy","root":ROOT,"from":"/escape/secret","to":"/stolen"}"#,
        r#"{"op":"rename","root":ROOT,"from":"/d","to":"/escape/d"}"#,
    ] {
        let err = e.run(op).unwrap_err();
        assert!(err.contains("link"), "{op}: {err}");
    }

    // Uploads into a linked folder, or over a link, never write outside.
    assert!(e.upload("/escape/planted", b"x", r#"{"mode":"always"}"#).is_err());
    assert!(e.upload("/d/file-link", b"x", r#"{"mode":"always"}"#).is_err());
    assert_eq!(std::fs::read(outside.path().join("secret")).unwrap(), b"top secret");
    assert!(!outside.path().join("planted").exists());

    // Deleting a link removes the link, not its target.
    e.run(r#"{"op":"delete","root":ROOT,"path":"/escape","permanent":true}"#).unwrap();
    assert!(outside.path().join("secret").exists());

    // stat reports the link itself.
    let s = e.run(r#"{"op":"stat","root":ROOT,"path":"/d/file-link"}"#).unwrap();
    assert_eq!(s["entry"]["kind"], "symlink");
}

#[cfg(windows)]
#[test]
fn rejects_windows_special_names() {
    use super::windows::valid_name;

    for bad in
        ["file.txt:secret", "CON", "con.txt", "LPT1", "COM9.log", "x.", "x ", "a*b", "nul", "PROGRA~3", "AGENT~1.TOM"]
    {
        assert!(valid_name(bad).is_err(), "{bad} accepted");
    }

    for ok in ["file.txt", "CONFIG", "COMPUTER.txt", ".env", "LPT", "notes~1.backup", "~1"] {
        assert!(valid_name(ok).is_ok(), "{ok} rejected");
    }
}

// ---- step 6: archives and permissions ------------------------------------

/// Builds a zip in memory with raw entry names (to forge hostile archives).
fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;
    let mut cur = std::io::Cursor::new(Vec::new());

    {
        let mut z = zip::ZipWriter::new(&mut cur);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (name, data) in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }

        z.finish().unwrap();
    }

    cur.into_inner()
}

#[test]
fn archive_names_are_checked() {
    use super::manage::safe_entry;
    assert_eq!(safe_entry("a/b/c.txt").unwrap(), ["a", "b", "c.txt"]);
    assert_eq!(safe_entry("./a//b").unwrap(), ["a", "b"]);

    for bad in ["../evil", "a/../../evil", "/etc/passwd", "C:/Windows/x", "a\\..\\..\\x", "x/.forge-upload-1.part"] {
        assert!(safe_entry(bad).is_err(), "{bad} accepted");
    }
}

#[test]
fn compress_and_extract_round_trip() {
    let e = Env::new();
    std::fs::create_dir_all(e.p("src/sub")).unwrap();
    std::fs::write(e.p("src/a.txt"), b"alpha").unwrap();
    std::fs::write(e.p("src/sub/b.txt"), b"beta").unwrap();

    for (fmt, name) in [("zip", "out.zip"), ("tar_gz", "out.tar.gz")] {
        if cfg!(windows) && fmt != "zip" {
            // Windows nodes create .zip only.
            let r = e.run(&format!(
                r#"{{"op":"compress","root":ROOT,"sources":["/src"],"dest":"/{name}","format":"{fmt}"}}"#
            ));
            assert!(r.is_err(), "{fmt} must be refused on Windows");

            continue;
        }

        let r = e
            .run(&format!(r#"{{"op":"compress","root":ROOT,"sources":["/src"],"dest":"/{name}","format":"{fmt}"}}"#))
            .unwrap();
        assert_eq!(r["entries"], 4, "{r}");

        // A finished archive gets the usual 0644, not the private 0600 it
        // is written with.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(e.p(name)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644, "{name}: {mode:o}");
        }
        // No overwrite.
        assert!(
            e.run(&format!(r#"{{"op":"compress","root":ROOT,"sources":["/src"],"dest":"/{name}","format":"{fmt}"}}"#))
                .is_err()
        );
        let dest = format!("x-{fmt}");
        let x = e.run(&format!(r#"{{"op":"extract","root":ROOT,"archive":"/{name}","dest":"/{dest}"}}"#)).unwrap();
        assert_eq!(x["bytes"], 9, "{x}");
        assert_eq!(std::fs::read(e.p(&format!("{dest}/src/sub/b.txt"))).unwrap(), b"beta");

        // The destination must be new.
        assert!(e.run(&format!(r#"{{"op":"extract","root":ROOT,"archive":"/{name}","dest":"/{dest}"}}"#)).is_err());
    }

    // A temporary archive for download, then read and delete it.
    let t = e.run(r#"{"op":"compress","root":ROOT,"sources":["/src/a.txt"],"format":"zip"}"#).unwrap();
    let id = t["temp_id"].as_str().unwrap().to_owned();
    let r = e.run(&format!(r#"{{"op":"temp_read","root":ROOT,"id":"{id}","length":100000}}"#)).unwrap();
    assert!(r["size"].as_u64().unwrap() > 0);

    // The store is shared by the node's roots: another root cannot open
    // or delete the archive by its id.
    let other = Root::at(e.base.path(), e.store.path()).unwrap();
    let spec = RootSpec::Dir { base: forge_core::files::FilePath::parse("/elsewhere").unwrap() };

    for op in [
        format!(r#"{{"op":"temp_read","root":{{"kind":"dir","base":"/elsewhere"}},"id":"{id}","length":10}}"#),
        format!(r#"{{"op":"temp_delete","root":{{"kind":"dir","base":"/elsewhere"}},"id":"{id}"}}"#),
    ] {
        let op: FileOp = serde_json::from_str(&op).unwrap();
        assert!(Ops::new(&other, &spec, false).run(&op).unwrap_err().contains("gone"));
    }

    assert!(e.run(&format!(r#"{{"op":"temp_read","root":ROOT,"id":"{id}","length":10}}"#)).is_ok());
    e.run(&format!(r#"{{"op":"temp_delete","root":ROOT,"id":"{id}"}}"#)).unwrap();
    assert!(e.run(&format!(r#"{{"op":"temp_read","root":ROOT,"id":"{id}","length":10}}"#)).is_err());
}

#[test]
fn hostile_archives_write_nothing_outside() {
    let e = Env::new();
    let outside = tempfile::tempdir().unwrap();

    for (i, name) in ["../escape.txt", "a/../../escape.txt", "/abs.txt"].iter().enumerate() {
        std::fs::write(e.p(&format!("evil{i}.zip")), zip_bytes(&[("ok.txt", b"ok"), (name, b"pwned")])).unwrap();
        let err =
            e.run(&format!(r#"{{"op":"extract","root":ROOT,"archive":"/evil{i}.zip","dest":"/out{i}"}}"#)).unwrap_err();
        assert!(err.contains("unsafe archive entry"), "{err}");

        // Nothing left behind: no destination, no partial folder.
        assert!(!e.p(&format!("out{i}")).exists());
        let leftovers: Vec<_> = std::fs::read_dir(e.base.path())
            .unwrap()
            .flatten()
            .filter(|d| d.file_name().to_string_lossy().starts_with(".forge-upload-"))
            .collect();
        assert!(leftovers.is_empty(), "partial folder left");
    }

    assert!(!e.base.path().parent().unwrap().join("escape.txt").exists());
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());

    // A zip bomb: 20 MiB of zeros compress to a few KiB.
    let zeros = vec![0u8; 20 * 1024 * 1024];
    std::fs::write(e.p("bomb.zip"), zip_bytes(&[("zeros.bin", &zeros)])).unwrap();
    let err = e.run(r#"{"op":"extract","root":ROOT,"archive":"/bomb.zip","dest":"/bomb"}"#).unwrap_err();
    assert!(err.contains("zip bomb"), "{err}");
    assert!(!e.p("bomb").exists());
}

#[cfg(unix)]
#[test]
fn tar_links_are_skipped_and_permissions_change() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();

    // A tar.gz with a symlink to /etc and a file.
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());

    {
        let mut t = tar::Builder::new(&mut gz);
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        t.append_link(&mut h, "etc-link", "/etc").unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(2);
        h.set_mode(0o4755);
        h.set_cksum();
        t.append_data(&mut h, "file.txt", &b"hi"[..]).unwrap();
        t.finish().unwrap();
    }

    std::fs::write(e.p("links.tar.gz"), gz.finish().unwrap()).unwrap();
    let r = e.run(r#"{"op":"extract","root":ROOT,"archive":"/links.tar.gz","dest":"/x"}"#).unwrap();
    assert_eq!(r["skipped"][0]["reason"], "link", "{r}");
    assert!(std::fs::symlink_metadata(e.p("x/etc-link")).is_err(), "link created");

    // setuid from the archive is not kept.
    let mode = std::fs::metadata(e.p("x/file.txt")).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o644);

    // chmod: dry run counts, then applies; links are skipped.
    std::os::unix::fs::symlink("/etc/hostname", e.p("x/link")).unwrap();
    let d = e
        .run(r#"{"op":"set_permissions","root":ROOT,"path":"/x","mode":448,"recursive":true,"dry_run":true}"#)
        .unwrap();
    assert_eq!(d["changed"], 2, "{d}");
    assert_eq!(std::fs::metadata(e.p("x/file.txt")).unwrap().permissions().mode() & 0o777, 0o644);
    let a = e.run(r#"{"op":"set_permissions","root":ROOT,"path":"/x","mode":448,"recursive":true}"#).unwrap();
    assert_eq!(a["skipped"], 1, "{a}");
    assert_eq!(std::fs::metadata(e.p("x/file.txt")).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(a["sample"][0]["after"], "0700");

    // A path through a link is an error, not a silent no-op.
    std::os::unix::fs::symlink("/etc", e.p("etc-link")).unwrap();
    let err = e.run(r#"{"op":"set_permissions","root":ROOT,"path":"/etc-link/passwd","mode":511}"#).unwrap_err();
    assert!(err.contains("link"), "{err}");
}

fn parts(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|d| d.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".forge-upload-"))
        .collect()
}

/// F-04: `parents` creates the whole path; without it the error names the
/// first missing folder in full.
#[test]
fn mkdir_with_parents_and_errors_that_name_the_missing_folder() {
    let e = Env::new();
    let err = e.run(r#"{"op":"mkdir","root":ROOT,"path":"/srv/forge/apps/site"}"#).unwrap_err();
    assert!(err.contains("parent folder /srv does not exist"), "{err}");
    let out = e.run(r#"{"op":"mkdir","root":ROOT,"path":"/srv/forge/apps/site","parents":true}"#).unwrap();
    assert_eq!(out["created"], 4);
    assert!(e.p("srv/forge/apps/site").is_dir());

    // Again: nothing to create, not an error.
    let out = e.run(r#"{"op":"mkdir","root":ROOT,"path":"/srv/forge/apps/site","parents":true}"#).unwrap();
    assert_eq!(out["created"], 0);

    // A file in the way is refused.
    std::fs::write(e.p("srv/file"), b"x").unwrap();
    let err = e.run(r#"{"op":"mkdir","root":ROOT,"path":"/srv/file/x","parents":true}"#).unwrap_err();
    assert!(err.contains("/srv/file") && err.contains("not a folder"), "{err}");

    // Uploads name the missing folder too.
    let err = e.upload("/srv/nope/deeper/f.txt", b"x", r#"{"mode":"never"}"#).unwrap_err();
    assert!(err.contains("/srv/nope/deeper") && err.contains("/srv/nope is missing"), "{err}");
}

/// F-07: `replace` swaps a new tree in for an existing folder; the default
/// still refuses, and a failed archive leaves the old folder untouched.
#[test]
fn extract_replace_swaps_the_folder() {
    let e = Env::new();
    std::fs::create_dir(e.p("site")).unwrap();
    std::fs::write(e.p("site/old.html"), b"old").unwrap();
    std::fs::write(e.p("v2.zip"), zip_bytes(&[("index.html", b"new"), ("css/a.css", b"a")])).unwrap();
    let err = e.run(r#"{"op":"extract","root":ROOT,"archive":"/v2.zip","dest":"/site"}"#).unwrap_err();
    assert!(err.contains("already exists"), "{err}");
    let out = e.run(r#"{"op":"extract","root":ROOT,"archive":"/v2.zip","dest":"/site","replace":true}"#).unwrap();
    assert_eq!(out["replaced"], true);
    assert_eq!(std::fs::read(e.p("site/index.html")).unwrap(), b"new");
    assert!(e.p("site/css/a.css").exists() && !e.p("site/old.html").exists());
    assert!(parts(e.base.path()).is_empty(), "old tree left behind");

    // A bad archive: the live folder stays as it was.
    std::fs::write(e.p("evil.zip"), zip_bytes(&[("ok.txt", b"ok"), ("../x.txt", b"x")])).unwrap();
    assert!(e.run(r#"{"op":"extract","root":ROOT,"archive":"/evil.zip","dest":"/site","replace":true}"#).is_err());
    assert_eq!(std::fs::read(e.p("site/index.html")).unwrap(), b"new");
    assert!(parts(e.base.path()).is_empty());

    // A file is never replaced by a folder.
    std::fs::write(e.p("file"), b"x").unwrap();
    assert!(e.run(r#"{"op":"extract","root":ROOT,"archive":"/v2.zip","dest":"/file","replace":true}"#).is_err());
}

/// F-08: a refused commit removes the partial file; an incomplete one keeps
/// it for resuming; abandoned partial files are swept.
#[test]
fn refused_uploads_leave_no_partial_file_and_stale_ones_are_swept() {
    let e = Env::new();
    std::fs::write(e.p("x.zip"), b"old").unwrap();
    let err = e.upload("/x.zip", b"new bytes", r#"{"mode":"never"}"#).unwrap_err();
    assert!(err.contains("already exists"), "{err}");
    assert!(parts(e.base.path()).is_empty(), "partial file left after a conflict");

    // A partial upload that is still resumable or retryable is kept.
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"abc");
    e.run(&format!(
        r#"{{"op":"upload_chunk","root":ROOT,"path":"/y.bin","upload":"u2","offset":0,"data_b64":"{b64}"}}"#
    ))
    .unwrap();
    let zero = "0".repeat(64);
    let commit = |size: u32| {
        format!(
            r#"{{"op":"upload_commit","root":ROOT,"path":"/y.bin","upload":"u2","size":{size},"sha256":"{zero}","overwrite":{{"mode":"never"}}}}"#
        )
    };

    // Incomplete: kept for resuming.
    assert!(e.run(&commit(10)).unwrap_err().contains("incomplete"));
    assert_eq!(parts(e.base.path()).len(), 1);

    // A checksum mismatch: kept, so the commit can be retried with the right checksum.
    assert!(e.run(&commit(3)).unwrap_err().contains("checksum"));
    assert_eq!(parts(e.base.path()).len(), 1);

    // The target appeared meanwhile: refused for good, partial file removed.
    std::fs::write(e.p("y.bin"), b"theirs").unwrap();
    let sha = super::hex(ring::digest::digest(&ring::digest::SHA256, b"abc").as_ref());
    let ok = commit(3).replace(&zero, &sha);
    assert!(e.run(&ok).unwrap_err().contains("already exists"));
    assert!(parts(e.base.path()).is_empty());

    // An abandoned partial file (not written for over a day) goes when the
    // next upload starts in that folder; a recent one stays.
    let stale = e.p(".forge-upload-dead.part");
    std::fs::write(&stale, b"left over").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600);
    std::fs::File::options().write(true).open(&stale).unwrap().set_modified(old).unwrap();
    std::fs::write(e.p(".forge-upload-live.part"), b"busy").unwrap();
    e.upload("/z.txt", b"z", r#"{"mode":"never"}"#).unwrap();
    assert!(!stale.exists());
    assert!(e.p(".forge-upload-live.part").exists());
}
