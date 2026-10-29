//! Picture list of one block: bumpers stay glued to their image, and a folder
//! named `blockN` fills the Nth upcoming occurrence of a program.

use std::cmp::Ordering;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::modes::Mode;
use crate::project::{BlockImage, ImageRole, Project, check_images, relativize, resolve};
use crate::schedule::upcoming_dates;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Pre,
    Post,
}

pub struct ListEdit {
    pub images: Vec<BlockImage>,
    pub warnings: Vec<String>,
}

struct Group {
    pre: Vec<BlockImage>,
    normal: BlockImage,
    post: Vec<BlockImage>,
}

fn groups(images: &[BlockImage]) -> anyhow::Result<Vec<Group>> {
    check_images(images)?;
    let mut out = Vec::new();
    let mut i = 0;
    while i < images.len() {
        let mut pre = Vec::new();
        while i < images.len() && images[i].role.is_pre() {
            pre.push(images[i].clone());
            i += 1;
        }
        let normal = images[i].clone();
        i += 1;
        let mut post = Vec::new();
        while i < images.len() && images[i].role.is_post() {
            post.push(images[i].clone());
            i += 1;
        }
        out.push(Group { pre, normal, post });
    }
    Ok(out)
}

fn flatten(groups: &[Group]) -> Vec<BlockImage> {
    let mut out = Vec::new();
    for g in groups {
        out.extend(g.pre.iter().cloned());
        out.push(g.normal.clone());
        out.extend(g.post.iter().cloned());
    }
    out
}

fn index_of(groups: &[Group], id: u64) -> Option<usize> {
    groups.iter().position(|g| g.normal.id == id)
}

/// `-bp` / `-bn` at the end of the file stem, before the extension. `foto-bp-2.png` does not match.
pub fn suffix_side(file_name: &str) -> Option<Side> {
    let stem = Path::new(file_name).file_stem()?.to_str()?.to_ascii_lowercase();
    if stem.ends_with("-bp") {
        Some(Side::Post)
    } else if stem.ends_with("-bn") {
        Some(Side::Pre)
    } else {
        None
    }
}

pub fn image_is_color(path: &Path) -> anyhow::Result<bool> {
    let img = image::ImageReader::open(path)?.with_guessed_format()?.decode()?.to_rgb8();
    Ok(img.pixels().any(|p| {
        let [r, g, b] = p.0;
        r.max(g).max(b) - r.min(g).min(b) > 8
    }))
}

/// Color picture → Robot 12 color. Grey picture → Robot 8. Unreadable → Robot 12.
pub fn bumper_mode(path: &Path) -> Mode {
    match image_is_color(path) {
        Ok(false) => Mode::RobotBw8,
        _ => Mode::Robot12,
    }
}

pub fn link_up(images: &[BlockImage], id: u64, mode_for: impl Fn(&str) -> Mode) -> anyhow::Result<Vec<BlockImage>> {
    let mut groups = groups(images)?;
    let idx = index_of(&groups, id).ok_or_else(|| anyhow::anyhow!("pick a normal image to link"))?;
    anyhow::ensure!(groups[idx].pre.is_empty() && groups[idx].post.is_empty(), "unlink this image's bumpers before joining it");
    anyhow::ensure!(idx > 0, "there is no image above");
    let mut image = groups.remove(idx).normal;
    let target = groups[idx - 1].normal.id;
    image.role = ImageRole::Post { of: target };
    image.mode = Some(mode_for(&image.path));
    groups[idx - 1].post.push(image);
    let out = flatten(&groups);
    check_images(&out)?;
    Ok(out)
}

pub fn link_down(images: &[BlockImage], id: u64, mode_for: impl Fn(&str) -> Mode) -> anyhow::Result<Vec<BlockImage>> {
    let mut groups = groups(images)?;
    let idx = index_of(&groups, id).ok_or_else(|| anyhow::anyhow!("pick a normal image to link"))?;
    anyhow::ensure!(groups[idx].pre.is_empty() && groups[idx].post.is_empty(), "unlink this image's bumpers before joining it");
    anyhow::ensure!(idx + 1 < groups.len(), "there is no image below");
    let mut image = groups.remove(idx).normal;
    let target = groups[idx].normal.id;
    image.role = ImageRole::Pre { of: target };
    image.mode = Some(mode_for(&image.path));
    groups[idx].pre.push(image);
    let out = flatten(&groups);
    check_images(&out)?;
    Ok(out)
}

pub fn unlink(images: &[BlockImage], id: u64) -> anyhow::Result<Vec<BlockImage>> {
    let mut groups = groups(images)?;
    let Some(idx) = groups.iter().position(|g| g.pre.iter().chain(g.post.iter()).any(|i| i.id == id)) else {
        anyhow::bail!("that image is not a bumper");
    };
    if let Some(pos) = groups[idx].pre.iter().position(|i| i.id == id) {
        let mut image = groups[idx].pre.remove(pos);
        image.role = ImageRole::Normal;
        groups.insert(idx, Group { pre: Vec::new(), normal: image, post: Vec::new() });
    } else if let Some(pos) = groups[idx].post.iter().position(|i| i.id == id) {
        let mut image = groups[idx].post.remove(pos);
        image.role = ImageRole::Normal;
        groups.insert(idx + 1, Group { pre: Vec::new(), normal: image, post: Vec::new() });
    }
    let out = flatten(&groups);
    check_images(&out)?;
    Ok(out)
}

/// Moves the whole group (pre + image + post). `to` is the group index after removal.
pub fn move_group(images: &[BlockImage], normal_id: u64, to: usize) -> anyhow::Result<Vec<BlockImage>> {
    let mut groups = groups(images)?;
    let from = index_of(&groups, normal_id).ok_or_else(|| anyhow::anyhow!("drag the group's normal image"))?;
    let group = groups.remove(from);
    let to = to.min(groups.len());
    groups.insert(to, group);
    Ok(flatten(&groups))
}

/// Reorders a bumper among the bumpers on its own side. `to` is the index on that side.
pub fn move_bumper(images: &[BlockImage], bumper_id: u64, to: usize) -> anyhow::Result<Vec<BlockImage>> {
    let mut groups = groups(images)?;
    for g in &mut groups {
        if let Some(pos) = g.pre.iter().position(|i| i.id == bumper_id) {
            let image = g.pre.remove(pos);
            let to = to.min(g.pre.len());
            g.pre.insert(to, image);
            return Ok(flatten(&groups));
        }
        if let Some(pos) = g.post.iter().position(|i| i.id == bumper_id) {
            let image = g.post.remove(pos);
            let to = to.min(g.post.len());
            g.post.insert(to, image);
            return Ok(flatten(&groups));
        }
    }
    anyhow::bail!("that image is not a bumper")
}

/// Appends files. `(stored path, file name)`. `-bp` joins the previous normal, `-bn` the next.
pub fn append_files(existing: &[BlockImage], files: &[(String, String)], mut alloc: impl FnMut() -> u64, mode_for: impl Fn(&str) -> Mode) -> anyhow::Result<ListEdit> {
    let mut groups = groups(existing)?;
    let existing_ids: Vec<u64> = groups.iter().map(|g| g.normal.id).collect();
    let mut warnings = Vec::new();
    let mut last_normal = groups.last().map(|g| g.normal.id);
    #[derive(Clone, Copy)]
    enum Mark {
        Normal,
        Pre,
        Post(u64),
        Pending,
    }
    let mut drafts: Vec<(BlockImage, Mark)> = Vec::new();
    for (path, name) in files {
        let id = alloc();
        let mut image = BlockImage::normal(id, path.clone());
        match suffix_side(name) {
            Some(Side::Post) => {
                if let Some(of) = last_normal {
                    image.role = ImageRole::Post { of };
                    image.mode = Some(mode_for(path));
                    drafts.push((image, Mark::Post(of)));
                } else {
                    warnings.push(format!("{name}: no hay imagen anterior; queda como imagen normal"));
                    last_normal = Some(id);
                    drafts.push((image, Mark::Normal));
                }
            }
            Some(Side::Pre) => drafts.push((image, Mark::Pending)),
            None => {
                last_normal = Some(id);
                drafts.push((image, Mark::Normal));
            }
        }
    }
    for i in 0..drafts.len() {
        if !matches!(drafts[i].1, Mark::Pending) {
            continue;
        }
        let next = drafts[i + 1..].iter().find(|(_, m)| matches!(m, Mark::Normal)).map(|(im, _)| im.id);
        let name = Path::new(&drafts[i].0.path).file_name().and_then(|s| s.to_str()).unwrap_or(&drafts[i].0.path).to_string();
        if let Some(of) = next {
            drafts[i].0.role = ImageRole::Pre { of };
            drafts[i].0.mode = Some(mode_for(&drafts[i].0.path));
            drafts[i].1 = Mark::Pre;
        } else {
            warnings.push(format!("{name}: no hay imagen siguiente; queda como imagen normal"));
            drafts[i].1 = Mark::Normal;
        }
    }
    let mut fresh = Vec::new();
    for (image, mark) in drafts {
        if let Mark::Post(of) = mark {
            if existing_ids.contains(&of) {
                if let Some(g) = groups.iter_mut().find(|g| g.normal.id == of) {
                    g.post.push(image);
                    continue;
                }
            }
        }
        fresh.push((image, mark));
    }
    let mut i = 0;
    while i < fresh.len() {
        let mut pre = Vec::new();
        while i < fresh.len() && matches!(fresh[i].1, Mark::Pre) {
            pre.push(fresh[i].0.clone());
            i += 1;
        }
        anyhow::ensure!(i < fresh.len() && matches!(fresh[i].1, Mark::Normal), "the image batch could not be grouped");
        let normal = fresh[i].0.clone();
        i += 1;
        let mut post = Vec::new();
        while i < fresh.len() && matches!(fresh[i].1, Mark::Post(of) if of == normal.id) {
            post.push(fresh[i].0.clone());
            i += 1;
        }
        groups.push(Group { pre, normal, post });
    }
    let images = flatten(&groups);
    check_images(&images)?;
    Ok(ListEdit { images, warnings })
}

pub struct ImportReport {
    pub filled: Vec<chrono::NaiveDate>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
}

fn block_number(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let rest = lower.strip_prefix("block")?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse::<u32>().ok().filter(|n| *n >= 1)
}

fn is_picture(name: &str) -> bool {
    matches!(
        Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "gif"
    )
}

/// `block01` is the next occurrence at or after `now`, counting empty ones. `block02` alone
/// fills the second and leaves the first alone.
pub fn import_program(project: &mut Project, program_id: u64, root: &Path, base: Option<&Path>, now: DateTime<Utc>) -> anyhow::Result<ImportReport> {
    anyhow::ensure!(project.program(program_id).is_some(), "that program is not in the channel");
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| anyhow::anyhow!("{}: {e}", root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(n) = block_number(&name) {
            dirs.push((n, entry.path(), name));
        }
    }
    dirs.sort_by_key(|(n, _, _)| *n);
    let dates = upcoming_dates(project, program_id, now);
    let mut report = ImportReport { filled: Vec::new(), skipped: Vec::new(), warnings: Vec::new() };
    for (n, path, name) in dirs {
        let Some(date) = dates.get((n as usize).saturating_sub(1)).copied() else {
            report.skipped.push(format!("{name}: the program has no occurrence {n}"));
            continue;
        };
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&path)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if is_picture(&file_name) {
                files.push((entry.path(), file_name));
            }
        }
        files.sort_by(|a, b| natural_cmp(&a.1, &b.1));
        if files.is_empty() {
            report.skipped.push(format!("{name}: has no images"));
            continue;
        }
        let stored: Vec<(String, String)> = files.iter().map(|(p, name)| (relativize(base, p), name.clone())).collect();
        let mut next = project.next_id();
        let edited = append_files(&[], &stored, || {
            let id = next;
            next += 1;
            id
        }, |stored| bumper_mode(&resolve(base, stored)))?;
        report.warnings.extend(edited.warnings);
        let block = project.ensure_program_block(program_id, date);
        block.images = edited.images;
        report.filled.push(date);
    }
    Ok(report)
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    #[derive(PartialEq)]
    enum Chunk {
        Num(u128),
        Text(String),
    }
    fn chunks(s: &str) -> Vec<Chunk> {
        let mut out = Vec::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.peek().copied() {
            if c.is_ascii_digit() {
                let mut n = 0u128;
                while let Some(d) = chars.peek().copied() {
                    if !d.is_ascii_digit() {
                        break;
                    }
                    chars.next();
                    n = n.saturating_mul(10).saturating_add(u128::from(d as u8 - b'0'));
                }
                out.push(Chunk::Num(n));
            } else {
                let mut t = String::new();
                while let Some(d) = chars.peek().copied() {
                    if d.is_ascii_digit() {
                        break;
                    }
                    t.push(chars.next().unwrap());
                }
                out.push(Chunk::Text(t));
            }
        }
        out
    }
    let (ac, bc) = (chunks(a), chunks(b));
    for (x, y) in ac.iter().zip(bc.iter()) {
        let ord = match (x, y) {
            (Chunk::Num(m), Chunk::Num(n)) => m.cmp(n),
            (Chunk::Text(m), Chunk::Text(n)) => m.to_ascii_lowercase().cmp(&n.to_ascii_lowercase()),
            (Chunk::Num(_), Chunk::Text(_)) => Ordering::Less,
            (Chunk::Text(_), Chunk::Num(_)) => Ordering::Greater,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    ac.len().cmp(&bc.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    use crate::project::{Block, Program};
    use chrono::NaiveDate;

    fn ids(images: &[BlockImage]) -> Vec<u64> {
        images.iter().map(|i| i.id).collect()
    }

    #[test]
    fn link_unlink_and_move_refuse_a_bridge() {
        let images = vec![BlockImage::normal(1, "a.png"), BlockImage::normal(2, "b.png"), BlockImage::normal(3, "c.png")];
        let mode = |_p: &str| Mode::Robot12;
        let up = link_up(&images, 2, mode).unwrap();
        assert!(matches!(up[1].role, ImageRole::Post { of: 1 }));
        assert_eq!(up[1].mode, Some(Mode::Robot12));
        let err = link_up(&up, 1, mode).unwrap_err().to_string();
        assert!(err.contains("unlink"), "{err}");
        let down = link_down(&images, 2, mode).unwrap();
        assert!(matches!(down[1].role, ImageRole::Pre { of: 3 }));
        let free = unlink(&down, 2).unwrap();
        assert!(free.iter().all(|i| matches!(i.role, ImageRole::Normal)));
        assert_eq!(ids(&free), [1, 2, 3], "a pre-bumper lands just before its group");
        let moved = move_group(&images, 1, 2).unwrap();
        assert_eq!(ids(&moved), [2, 3, 1]);
    }

    #[test]
    fn suffixes_and_the_orphan_cases() {
        assert_eq!(suffix_side("foto-bp.PNG"), Some(Side::Post));
        assert_eq!(suffix_side("foto-BN.jpg"), Some(Side::Pre));
        assert_eq!(suffix_side("foto-bp-2.png"), None);
        let existing = vec![BlockImage::normal(1, "ya.png")];
        let files = vec![
            ("a-bn.png".into(), "a-bn.png".into()),
            ("b.png".into(), "b.png".into()),
            ("c-bp.png".into(), "c-bp.png".into()),
            ("solo-bp.png".into(), "solo-bp.png".into()),
        ];
        // solo-bp has b as the previous normal, so it is a post of b, not an orphan.
        // An orphan post is one that is first.
        let orphan = append_files(&[], &[("z-bp.png".into(), "z-bp.png".into()), ("z-bn.png".into(), "z-bn.png".into())], {
            let mut n = 1;
            move || {
                let id = n;
                n += 1;
                id
            }
        }, |_| Mode::RobotBw8)
        .unwrap();
        assert_eq!(orphan.warnings.len(), 2, "{:?}", orphan.warnings);
        assert!(orphan.images.iter().all(|i| matches!(i.role, ImageRole::Normal)));

        let added = append_files(&existing, &files[..3], {
            let mut n = 10;
            move || {
                let id = n;
                n += 1;
                id
            }
        }, |_| Mode::Robot12)
        .unwrap();
        assert!(added.warnings.is_empty(), "{:?}", added.warnings);
        assert!(matches!(added.images[1].role, ImageRole::Pre { of: 11 }));
        assert_eq!(added.images[2].id, 11);
        assert!(matches!(added.images[3].role, ImageRole::Post { of: 11 }));
        // -bp onto the picture already in the list.
        let onto = append_files(&existing, &[("x-bp.png".into(), "x-bp.png".into())], || 8, |_| Mode::RobotBw8).unwrap();
        assert!(matches!(onto.images[1].role, ImageRole::Post { of: 1 }));
        assert!(onto.warnings.is_empty());
    }

    #[test]
    fn natural_order_and_block02_leaves_the_first_empty() {
        let names = ["img10.png", "img2.png", "IMG1.png"];
        let mut v = names.to_vec();
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["IMG1.png", "img2.png", "img10.png"]);

        let dir = std::env::temp_dir().join(format!("estivi-imp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("block02")).unwrap();
        std::fs::write(dir.join("block02/img10.png"), b"not really").ok();
        std::fs::write(dir.join("block02/img2.png"), b"not really").ok();
        // A 1x1 png so the color probe can open it. Grey.
        let png = tiny_png();
        std::fs::write(dir.join("block02/img2.png"), &png).unwrap();
        std::fs::write(dir.join("block02/img10.png"), &png).unwrap();
        let mut days = [false; 7];
        days[0] = true;
        days[1] = true;
        let mut project = Project::default();
        let mut news = Program { id: 1, name: "Noticias".into(), weekdays: days, ..Program::default() };
        news.every_day("14:00", "14:30", 6);
        project.programs.push(news);
        // Monday 2026-10-26 15:00Z is after 14:00, so the next Monday occurrence is Nov 2,
        // and the second is Nov 3? Weekdays are Mon and Tue. From Monday 15:00 the next
        // start >= now is Tuesday Oct 27, then Monday Nov 2.
        let now = DateTime::parse_from_rfc3339("2026-10-26T15:00:00Z").unwrap().with_timezone(&Utc);
        let report = import_program(&mut project, 1, &dir, Some(&dir), now).unwrap();
        assert_eq!(report.filled, vec![NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()]);
        assert!(project.program_block(1, NaiveDate::from_ymd_opt(2026, 10, 27).unwrap()).is_none());
        let block = project.program_block(1, report.filled[0]).unwrap();
        let names: Vec<_> = block.images.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(names, ["block02/img2.png", "block02/img10.png"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tiny_png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([20, 20, 20]));
        let mut buf = Vec::new();
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(img.as_raw(), 2, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        buf
    }

    #[test]
    fn emptying_a_program_block_removes_it() {
        let mut project = Project::default();
        project.blocks.push(Block {
            id: 1,
            program: Some(2),
            date: NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(),
            images: vec![BlockImage::normal(3, "a.png")],
            ..Block::default()
        });
        project.blocks[0].images.clear();
        project.normalize().unwrap();
        assert!(project.blocks.is_empty());
    }
}
