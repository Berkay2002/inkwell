//! Media listing and the media route on generated files: what plays, what is only listed, what is
//! refused, and that nothing here touches the document.

use super::route::{self, CHUNK};
use super::*;
use crate::engine::cache::TileKey;
use std::path::PathBuf;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("media-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-page PDF with the given extra objects (numbered from 10) and page /Annots.
fn pdf(annots: &[u32], objs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let list: String = annots.iter().map(|n| format!("{n} 0 R ")).collect();
    let mut all: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (3, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{list}] >>").into_bytes()),
    ];
    all.extend(objs.iter().cloned());
    all.sort_by_key(|o| o.0);
    let max = all.last().unwrap().0;
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = vec![0usize; max as usize + 1];
    for (n, body) in &all {
        offs[*n as usize] = out.len();
        out.extend(format!("{n} 0 obj\n").as_bytes());
        out.extend(body);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", max + 1).as_bytes());
    for n in 1..=max {
        match offs[n as usize] {
            0 => out.extend(b"0000000000 65535 f \n"),
            o => out.extend(format!("{o:010} 00000 n \n").as_bytes()),
        }
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", max + 1).as_bytes());
    out
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut b = format!("<< /Length {} {dict} >>\nstream\n", data.len()).into_bytes();
    b.extend(data);
    b.extend(b"\nendstream");
    b
}

fn spec(name: &str, ef: u32) -> Vec<u8> {
    format!("<< /Type /Filespec /F ({name}) /UF ({name}) /EF << /F {ef} 0 R >> >>").into_bytes()
}

fn mp4(len: usize) -> Vec<u8> {
    let mut v = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom".to_vec();
    v.resize(len.max(24), 7);
    v
}

fn mp3() -> Vec<u8> {
    let mut v = b"ID3\x03\0\0\0\0\0\0".to_vec();
    v.resize(2048, 0x55);
    v
}

fn swf() -> Vec<u8> {
    let mut v = b"FWS\x09\x40\0\0\0".to_vec();
    v.resize(512, 1);
    v
}

fn pe() -> Vec<u8> {
    let mut v = b"MZ\x90\0\x03\0\0\0".to_vec();
    v.resize(512, 2);
    v
}

/// The sample: RichMedia (mp4 + swf + js), Screen (mp3 rendition with a script), Movie (outside
/// file), 3D (with a red poster), a "video/mp4" that is a PE file, and raw sound samples.
fn sample() -> Vec<u8> {
    let objs: Vec<(u32, Vec<u8>)> = vec![
        // RichMedia with an mp4 and a Flash player in its asset tree
        (10, b"<< /Type /Annot /Subtype /RichMedia /Rect [100 600 300 700] /T (Intro) /RichMediaContent << /Assets << /Names [(clip.mp4) 11 0 R (player.swf) 12 0 R (run.js) 13 0 R] >> /Configurations [<< /Subtype /Video /Instances [<< /Subtype /Video /Asset 11 0 R >>] >>] >> >>".to_vec()),
        (11, spec("clip.mp4", 21)),
        (12, spec("player.swf", 22)),
        (13, spec("run.js", 23)),
        (21, stream("/Type /EmbeddedFile /Subtype /video#2Fmp4", &mp4(4000))),
        (22, stream("/Type /EmbeddedFile /Subtype /application#2Fx-shockwave-flash", &swf())),
        (23, stream("/Type /EmbeddedFile", b"app.alert('hi');")),
        // Screen with a media rendition carrying an mp3, and a script
        (14, b"<< /Type /Annot /Subtype /Screen /Rect [100 400 300 500] /T (Voice) /A << /S /Rendition /OP 0 /JS (app.alert\\(1\\)) /R << /S /MR /C << /S /MCD /CT (audio/mpeg) /D 15 0 R >> >> >> >>".to_vec()),
        (15, spec("voice.mp3", 24)),
        (24, stream("/Type /EmbeddedFile", &mp3())),
        // Movie whose file is outside the document
        (16, b"<< /Type /Annot /Subtype /Movie /Rect [100 200 300 300] /Movie << /F << /Type /Filespec /F (C:\\\\videos\\\\talk.mp4) >> >> >>".to_vec()),
        // 3D: not supported, its appearance is the poster (a red box)
        (17, b"<< /Type /Annot /Subtype /3D /Rect [350 600 550 700] /Contents (Engine) /3DD 25 0 R /AP << /N 26 0 R >> /F 4 >>".to_vec()),
        (25, stream("/Type /3D /Subtype /U3D", b"U3D\0\x18\0\0\0")),
        (26, stream("/Type /XObject /Subtype /Form /BBox [0 0 200 100]", b"1 0 0 rg 0 0 200 100 re f")),
        // declared video/mp4, but the bytes are a Windows program
        (18, b"<< /Type /Annot /Subtype /Movie /Rect [350 400 550 500] /Movie << /F 19 0 R >> >>".to_vec()),
        (19, spec("evil.mp4", 27)),
        (27, stream("/Type /EmbeddedFile /Subtype /video#2Fmp4", &pe())),
        // a sound annotation: raw samples
        (20, b"<< /Type /Annot /Subtype /Sound /Rect [350 200 400 250] /Sound 28 0 R >>".to_vec()),
        (28, stream("/Type /Sound /R 44100 /C 1 /B 16 /E /Signed", &[0u8; 400])),
    ];
    pdf(&[10, 14, 16, 17, 18, 20], &objs)
}

fn open(test: &str, bytes: Vec<u8>) -> (Arc<Engine>, DocId, PathBuf) {
    let p = dir(test).join("a.pdf");
    std::fs::write(&p, bytes).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    (eng, id, p)
}

fn by_kind(items: &[MediaItem], k: Kind) -> &MediaItem {
    items.iter().find(|i| i.kind == k).unwrap_or_else(|| panic!("no {k:?}"))
}

#[test]
fn rich_media_lists_its_assets_and_plays_only_the_mp4() {
    let (eng, id, _) = open("rich", sample());
    let items = list_all(&eng, id).unwrap();
    let r = by_kind(&items, Kind::RichMedia);
    assert_eq!((r.page, r.title.as_str()), (0, "Intro"));
    assert!(r.playable && r.scripts);
    let name = |n: &str| r.assets.iter().find(|a| a.name == n).unwrap_or_else(|| panic!("no {n}"));
    let clip = name("clip.mp4");
    assert!(clip.playable && clip.embedded && clip.mime.as_deref() == Some("video/mp4") && clip.element == Some(Element::Video));
    assert_eq!(r.play.as_deref(), Some(clip.id.as_str()));
    assert!(clip.size >= 4000);
    let swf = name("player.swf");
    assert!(!swf.playable && swf.reason.as_deref().unwrap().contains("Flash"), "{swf:?}");
    assert!(!name("run.js").playable && name("run.js").reason.as_deref().unwrap().contains("script"));
    // the same file spec is listed once
    assert_eq!(r.assets.len(), 3);
    // the rect is where the annotation sits on the page (y down)
    assert_eq!(r.rect.map(|v| v.round()), [100.0, 92.0, 300.0, 192.0]);
}

#[test]
fn screen_rendition_with_an_mp3_plays_and_its_script_is_flagged() {
    let (eng, id, _) = open("screen", sample());
    let items = list_all(&eng, id).unwrap();
    let s = by_kind(&items, Kind::Screen);
    assert_eq!(s.title, "Voice");
    assert!(s.playable && s.scripts);
    let a = &s.assets[0];
    assert_eq!((a.name.as_str(), a.mime.as_deref(), a.element), ("voice.mp3", Some("audio/mpeg"), Some(Element::Audio)));
}

#[test]
fn an_outside_movie_is_listed_and_never_loaded() {
    let (eng, id, _) = open("movie", sample());
    let items = list_all(&eng, id).unwrap();
    let m = items.iter().find(|i| i.kind == Kind::Movie && i.assets[0].name.contains("talk")).unwrap();
    assert!(!m.playable && m.play.is_none());
    assert!(!m.assets[0].embedded);
    assert_eq!(m.reason.as_deref(), Some("external media, not loaded"));
    assert_eq!(m.assets[0].name, "talk.mp4", "no folder in the name");
}

#[test]
fn three_d_is_not_supported_and_its_poster_is_drawn() {
    let (eng, id, _) = open("3d", sample());
    let items = list_all(&eng, id).unwrap();
    let d = by_kind(&items, Kind::ThreeD);
    assert!(!d.playable);
    assert_eq!(d.reason.as_deref(), Some("3D content is not supported"));
    assert_eq!(d.title, "Engine");
    // the page tile shows the appearance stream: red inside the rect, white outside
    let entry = eng.doc_entry(id).unwrap();
    let t = eng.render_tile(&entry, &TileKey { doc: id, rev: 0, page: 0, rot: 0, scale: 1000, tx: 0, ty: 0 }).unwrap();
    let px = |x: usize, y: usize| {
        let i = (y * t.w as usize + x) * 4;
        [t.rgba[i], t.rgba[i + 1], t.rgba[i + 2]]
    };
    assert_eq!(px(450, 140), [255, 0, 0], "inside the 3D rect (x 350..550, y 92..192)");
    assert_eq!(px(450, 250), [255, 255, 255]);
}

#[test]
fn a_declared_mp4_that_is_a_program_is_not_playable() {
    let (eng, id, _) = open("pe", sample());
    let items = list_all(&eng, id).unwrap();
    let e = items.iter().find(|i| i.assets.first().is_some_and(|a| a.name == "evil.mp4")).unwrap();
    assert!(!e.playable);
    assert!(e.assets[0].reason.as_deref().unwrap().contains("executable"), "{:?}", e.assets[0]);
    assert!(e.assets[0].mime.is_none());
    // and the route refuses it by id
    let c = Cache::default();
    assert_eq!(serve(&eng, &c, id, &e.assets[0].id, None).status, 404);
}

#[test]
fn raw_sound_samples_are_not_playable() {
    let (eng, id, _) = open("sound", sample());
    let items = list_all(&eng, id).unwrap();
    let s = by_kind(&items, Kind::Sound);
    assert!(!s.playable);
    assert!(s.reason.as_deref().unwrap().contains("raw samples"));
}

#[test]
fn a_declared_type_must_match_the_bytes() {
    // an mp3 declared as video/mp4 (and named .mp4)
    let objs = vec![
        (10, b"<< /Type /Annot /Subtype /Movie /Rect [0 0 50 50] /Movie << /F 11 0 R >> >>".to_vec()),
        (11, spec("fake.mp4", 12)),
        (12, stream("/Type /EmbeddedFile /Subtype /video#2Fmp4", &mp3())),
    ];
    let (eng, id, _) = open("mismatch", pdf(&[10], &objs));
    let m = &list_all(&eng, id).unwrap()[0];
    assert!(!m.playable);
    assert!(m.reason.as_deref().unwrap().starts_with("declared as"), "{:?}", m.reason);
}

#[test]
fn the_route_serves_byte_ranges_and_refuses_unknown_ids() {
    let (eng, id, _) = open("route", sample());
    let items = list_all(&eng, id).unwrap();
    let play = by_kind(&items, Kind::RichMedia).play.clone().unwrap();
    let c = Cache::default();
    let want = {
        let Loaded { bytes, .. } = eng.read_custom(id, |d| read(d, &play)).unwrap();
        bytes
    };
    let header = |r: &route::Reply, k: &str| r.headers.iter().find(|h| h.0 == k).map(|h| h.1.clone());

    let r = serve(&eng, &c, id, &play, None);
    assert_eq!((r.status, header(&r, "Content-Type").as_deref()), (200, Some("video/mp4")));
    assert_eq!(r.body, *want);
    assert_eq!(header(&r, "X-Content-Type-Options").as_deref(), Some("nosniff"));
    assert_eq!(header(&r, "Accept-Ranges").as_deref(), Some("bytes"));

    let r = serve(&eng, &c, id, &play, Some("bytes=10-19"));
    assert_eq!(r.status, 206);
    assert_eq!(header(&r, "Content-Range"), Some(format!("bytes 10-19/{}", want.len())));
    assert_eq!(r.body, want[10..20]);

    let r = serve(&eng, &c, id, &play, Some("bytes=-8"));
    assert_eq!((r.status, r.body), (206, want[want.len() - 8..].to_vec()));

    let r = serve(&eng, &c, id, &play, Some("bytes=3000-"));
    assert_eq!(r.status, 206);
    assert_eq!(r.body, want[3000..]);

    let r = serve(&eng, &c, id, &play, Some(&format!("bytes={}-", want.len())));
    assert_eq!(r.status, 416);
    assert_eq!(header(&r, "Content-Range"), Some(format!("bytes */{}", want.len())));

    // not an asset, not a media object, not playable, not an open document
    for bad in ["999.0", "10.9", "10.1", "10.2", "3.0", "x.y", "0.0", "-5.0"] {
        assert_eq!(serve(&eng, &c, id, bad, None).status, 404, "{bad}");
    }
    assert_eq!(serve(&eng, &c, id + 100, &play, None).status, 404);
}

#[test]
fn open_ended_ranges_are_cut_to_a_chunk_and_cover_the_file_piece_by_piece() {
    let data: Vec<u8> = (0..CHUNK * 2 + 123).map(|i| (i % 251) as u8).collect();
    let r = route::respond(&data, "video/mp4", Some("bytes=0-"));
    assert_eq!((r.status, r.body.len()), (206, CHUNK));
    let mut got = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let r = route::respond(&data, "video/mp4", Some(&format!("bytes={at}-")));
        assert_eq!(r.status, 206);
        at += r.body.len();
        got.extend(r.body);
    }
    assert_eq!(got, data);
    // several ranges or bad syntax: the whole body
    assert_eq!(route::respond(&data, "video/mp4", Some("bytes=0-1,5-6")).status, 200);
    assert_eq!(route::respond(&data, "video/mp4", Some("bytes=9-3")).status, 200);
    assert_eq!(route::respond(&data, "video/mp4", Some("bytes=-0")).status, 416);
    assert_eq!(route::respond(&[], "video/mp4", Some("bytes=0-")).status, 416);
}

#[test]
fn the_route_path_takes_only_ids() {
    assert_eq!(route::parse("/m/3/10.0"), Some((3, "10.0".into())));
    for p in ["/t/3/10.0", "/m/3", "/m/3/10", "/m/3/10.0/x", "/m/a/10.0", "/m/3/../10.0", "/m/3/C:/x.mp4", "/m/3/10.0%2e", "/m//10.0", "/m/3/.0"] {
        assert_eq!(route::parse(p), None, "{p}");
    }
}

#[test]
fn a_hostile_name_tree_is_bounded() {
    // /Kids that list themselves many times, and a very wide leaf
    let kids: String = (0..5000).map(|_| "11 0 R ").collect();
    let mut names = String::new();
    for i in 0..20_000 {
        names.push_str(&format!("(f{i}.txt) 12 0 R "));
    }
    let objs = vec![
        (10, b"<< /Type /Annot /Subtype /RichMedia /Rect [0 0 50 50] /RichMediaContent << /Assets 11 0 R >> >>".to_vec()),
        (11, format!("<< /Kids [13 0 R {kids}] >>").into_bytes()),
        (12, spec("x.txt", 14)),
        (13, format!("<< /Names [{names}] >>").into_bytes()),
        (14, stream("/Type /EmbeddedFile", b"hello")),
    ];
    let (eng, id, _) = open("hostile", pdf(&[10], &objs));
    let t = std::time::Instant::now();
    let items = list_all(&eng, id).unwrap();
    assert!(t.elapsed().as_secs() < 20, "bounded work");
    assert_eq!(items.len(), 1);
    // one file spec behind every name: listed once
    assert_eq!(items[0].assets.len(), 1);
    assert!(!items[0].playable);
}

#[test]
fn many_media_annotations_on_a_page_are_capped() {
    let n = MAX_PAGE_ITEMS + 50;
    let mut ids = Vec::new();
    let mut objs = Vec::new();
    for i in 0..n as u32 {
        ids.push(100 + i);
        objs.push((100 + i, b"<< /Type /Annot /Subtype /Screen /Rect [0 0 10 10] >>".to_vec()));
    }
    let (eng, id, _) = open("many", pdf(&ids, &objs));
    assert_eq!(list_all(&eng, id).unwrap().len(), MAX_PAGE_ITEMS);
}

#[test]
fn listing_and_serving_leave_the_document_alone() {
    let (eng, id, path) = open("unchanged", sample());
    let before = (std::fs::read(&path).unwrap(), eng.edit_state(id).unwrap().rev, eng.history(id).unwrap().len());
    let items = list_all(&eng, id).unwrap();
    let c = Cache::default();
    for a in items.iter().flat_map(|i| i.assets.iter()) {
        let _ = serve(&eng, &c, id, &a.id, Some("bytes=0-99"));
    }
    let after = (std::fs::read(&path).unwrap(), eng.edit_state(id).unwrap().rev, eng.history(id).unwrap().len());
    assert!(before == after, "file, revision and history are unchanged");
    assert!(!eng.edit_state(id).unwrap().dirty, "no unsaved edits");
}

/// The committed sample (scripts/media-sample.py): a real H.264 mp4 and a real mp3.
#[test]
fn the_sample_file_lists_what_the_windows_check_expects() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/study/samples/media-source.pdf");
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(path).unwrap().doc;
    let items = list_all(&eng, id).unwrap();
    assert_eq!(items.len(), 4, "{items:#?}");
    let video = by_kind(&items, Kind::RichMedia);
    assert_eq!((video.page, video.title.as_str()), (0, "Test video"));
    let a = video.assets.iter().find(|a| a.name == "clip.mp4").unwrap();
    assert!(a.playable && a.mime.as_deref() == Some("video/mp4"), "{a:?}");
    assert!(video.assets.iter().any(|a| a.name == "player.swf" && !a.playable));
    let audio = by_kind(&items, Kind::Screen);
    assert!(audio.playable && audio.assets[0].mime.as_deref() == Some("audio/mpeg"), "{audio:?}");
    assert!(!by_kind(&items, Kind::ThreeD).playable);
    assert!(!by_kind(&items, Kind::Movie).playable);
    // the route plays the whole mp4: ranges stitch back to the stream
    let cache = Cache::default();
    let r = serve(&eng, &cache, id, &video.play.clone().unwrap(), Some("bytes=0-"));
    assert_eq!(r.status, 206);
    assert_eq!(&r.body[4..8], b"ftyp");
}

#[test]
fn pages_sharing_one_huge_annots_array_cost_one_budget() {
    let pages = 100;
    let entries = 20_000;
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = vec![0usize; pages + 6];
    let mut obj = |out: &mut Vec<u8>, n: usize, body: String| {
        offs[n] = out.len();
        out.extend(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    };
    obj(&mut out, 1, "<< /Type /Catalog /Pages 2 0 R >>".into());
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 6 + i)).collect();
    obj(&mut out, 2, format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"));
    obj(&mut out, 3, "<< /Type /Annot /Subtype /Screen /Rect [0 0 10 10] >>".into());
    obj(&mut out, 4, "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] >>".into());
    let list: String = std::iter::once("3 0 R ".to_string()).chain((0..entries).map(|_| "4 0 R ".to_string())).collect();
    obj(&mut out, 5, format!("[{list}]"));
    for i in 0..pages {
        obj(&mut out, 6 + i, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots 5 0 R >>".into());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", pages + 6).as_bytes());
    for n in 1..pages + 6 {
        out.extend(format!("{:010} 00000 n \n", offs[n]).as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", pages + 6).as_bytes());
    let (eng, id, _) = open("shared-annots", out);
    let t = std::time::Instant::now();
    let items = list_all(&eng, id).unwrap();
    // 50 000 loaded annotations / 20 001 per page: two pages are loaded, the rest are left out
    assert_eq!(items.len(), 2);
    assert!(t.elapsed().as_secs() < 30, "{:?}", t.elapsed());
}
