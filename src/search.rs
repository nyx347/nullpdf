//! Searching: full-text search over the document's pages, and filtering the outline
//! (bookmarks) by title.
//!
//! Text search is incremental. Pdfium does the matching one page at a time
//! ([`crate::pdf::PdfDoc::find_text`]), a few pages per frame, starting where the reader is.
//! The current match is chosen as soon as the pages that decide it have been searched, so
//! the first hit after the view appears at once even in a long document, and a next/previous
//! that runs ahead of the search waits for exactly the pages it needs.

use std::cmp::Reverse;
use std::ops::Range;

use egui::Rect;
use icu_normalizer::DecomposingNormalizerBorrowed;

use crate::pdf::OutlineItem;

/// One page's matches.
#[derive(Default)]
pub struct PageHits {
    /// Rectangles of every match (one per line a match spans), page-local points (top-left
    /// origin).
    pub rects: Vec<Rect>,
    /// The `rects` each match covers, in the page's text order.
    pub hits: Vec<Range<usize>>,
}

impl PageHits {
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// The rectangles of match `i`.
    pub fn hit(&self, i: usize) -> &[Rect] {
        &self.rects[self.hits[i].clone()]
    }

    /// Bounding box of match `i`.
    pub fn bbox(&self, i: usize) -> Rect {
        self.hit(i)
            .iter()
            .copied()
            .reduce(Rect::union)
            .unwrap_or(Rect::NOTHING)
    }
}

/// A match: its page, and its index among that page's matches.
pub type HitRef = (usize, usize);

/// Where a walk through the results ends.
enum Walk {
    Found(HitRef),
    /// The answer depends on this page, which hasn't been searched yet.
    Need(usize),
    /// There are no matches anywhere.
    Nothing,
}

/// A text search in progress (or finished) over every page.
pub struct TextSearch {
    pub query: String,
    pub match_case: bool,
    /// Per page: `None` until searched.
    pages: Vec<Option<PageHits>>,
    searched: usize,
    total: usize,
    /// The reading position the search started from: a page, and how far down it (points).
    /// The first match at or after it becomes current.
    anchor: (usize, f32),
    pub current: Option<HitRef>,
    /// Next (+) / previous (−) steps requested but not yet taken, waiting on unsearched pages.
    pending: i32,
}

impl TextSearch {
    pub fn new(query: String, match_case: bool, page_count: usize, anchor: (usize, f32)) -> Self {
        Self {
            query,
            match_case,
            pages: (0..page_count).map(|_| None).collect(),
            searched: 0,
            total: 0,
            anchor,
            current: None,
            pending: 0,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.searched == self.pages.len()
    }

    /// Matches found so far.
    pub fn total(&self) -> usize {
        self.total
    }

    /// The current match's 1-based position among all matches, once every page is searched.
    pub fn position(&self) -> Option<usize> {
        let (page, i) = self.current?;
        if !self.is_complete() {
            return None;
        }
        let before: usize = self.pages[..page].iter().flatten().map(PageHits::len).sum();
        Some(before + i + 1)
    }

    /// Page `i`'s matches, if it has been searched.
    pub fn page(&self, i: usize) -> Option<&PageHits> {
        self.pages.get(i)?.as_ref()
    }

    /// The page to search next: one the current match (or a pending step) waits on, else the
    /// next unsearched page onward from the anchor.
    pub fn next_page(&self) -> Option<usize> {
        let walk = match self.current {
            None => self.walk_first(),
            Some(cur) if self.pending != 0 => self.walk_step(cur, self.pending.signum()),
            Some(_) => Walk::Nothing,
        };
        if let Walk::Need(p) = walk {
            return Some(p);
        }
        let n = self.pages.len();
        (0..n)
            .map(|k| (self.anchor.0 + k) % n)
            .find(|&p| self.pages[p].is_none())
    }

    /// Store page `i`'s matches. Returns `true` if that moved the current match.
    pub fn add_page(&mut self, i: usize, hits: PageHits) -> bool {
        if self.pages[i].is_some() {
            return false;
        }
        self.total += hits.len();
        self.searched += 1;
        self.pages[i] = Some(hits);
        self.settle()
    }

    /// Step to the next (`dir > 0`) or previous match, wrapping around the document. Returns
    /// `true` if the current match moved now; otherwise the step waits for pages still to be
    /// searched. Does nothing until a first match has been found.
    pub fn step(&mut self, dir: i32) -> bool {
        if self.current.is_none() {
            return false;
        }
        self.pending += dir.signum();
        self.settle()
    }

    /// Start from the last match *before* the anchor instead of the first one after it.
    pub fn start_backward(&mut self) {
        self.pending = -1;
    }

    /// Resolve the current match and pending steps as far as the searched pages allow.
    fn settle(&mut self) -> bool {
        let mut moved = false;
        if self.current.is_none() {
            match self.walk_first() {
                Walk::Found(h) => {
                    self.current = Some(h);
                    moved = true;
                }
                Walk::Need(_) => return false,
                Walk::Nothing => {
                    self.pending = 0;
                    return false;
                }
            }
        }
        while let Some(cur) = self.current.filter(|_| self.pending != 0) {
            let dir = self.pending.signum();
            match self.walk_step(cur, dir) {
                Walk::Found(h) => {
                    self.current = Some(h);
                    self.pending -= dir;
                    moved = true;
                }
                Walk::Need(_) => break,
                Walk::Nothing => self.pending = 0,
            }
        }
        moved
    }

    /// The first match at or after the anchor, wrapping round to the anchor page's top.
    fn walk_first(&self) -> Walk {
        let n = self.pages.len();
        let (start, y) = self.anchor;
        if n == 0 {
            return Walk::Nothing;
        }
        for k in 0..=n {
            let p = (start + k) % n;
            let Some(hits) = &self.pages[p] else {
                return Walk::Need(p);
            };
            let found = if k == 0 {
                (0..hits.len()).find(|&i| hits.bbox(i).center().y >= y)
            } else {
                (!hits.is_empty()).then_some(0)
            };
            if let Some(i) = found {
                return Walk::Found((p, i));
            }
        }
        Walk::Nothing
    }

    /// The match after (`dir > 0`) or before `from`, wrapping around the document.
    fn walk_step(&self, (page, i): HitRef, dir: i32) -> Walk {
        let n = self.pages.len();
        let len = self.page(page).map_or(0, PageHits::len);
        if dir > 0 && i + 1 < len {
            return Walk::Found((page, i + 1));
        }
        if dir < 0 && i > 0 {
            return Walk::Found((page, i - 1));
        }
        for k in 1..=n {
            let p = (page as isize + dir as isize * k as isize).rem_euclid(n as isize) as usize;
            let Some(hits) = &self.pages[p] else {
                return Walk::Need(p);
            };
            if !hits.is_empty() {
                return Walk::Found((p, if dir > 0 { 0 } else { hits.len() - 1 }));
            }
        }
        Walk::Nothing
    }
}

/// A character as matched: lower-cased (one-to-one, so positions carry over to the original
/// text) and, for a letter with accents, the letter it's built on.
#[derive(Clone, Copy)]
struct Folded {
    lower: char,
    base: char,
}

impl Folded {
    fn of(c: char) -> Self {
        let lower = c.to_lowercase().next().unwrap_or(c);
        // é → e + ◌́: keep the letter if all that follows it is diacritical marks.
        let mut parts =
            DecomposingNormalizerBorrowed::new_nfd().normalize_iter(std::iter::once(lower));
        let first = parts.next().unwrap_or(lower);
        let base = if parts.all(|m| ('\u{300}'..='\u{36f}').contains(&m)) {
            first
        } else {
            lower
        };
        Self { lower, base }
    }

    /// Whether query character `self` matches text character `t`. A plain letter matches its
    /// accented forms too ("e" finds "é"); an accented one only itself.
    fn matches(self, t: Folded) -> bool {
        self.lower == t.lower || (self.lower == self.base && self.lower == t.base)
    }
}

fn folded(s: &str) -> Vec<Folded> {
    s.chars().map(Folded::of).collect()
}

/// Where `needle` occurs in `hay`.
fn occurrences<'a>(hay: &'a [Folded], needle: &'a [Folded]) -> impl Iterator<Item = usize> + 'a {
    (0..(hay.len() + 1).saturating_sub(needle.len()))
        .filter(move |&s| needle.iter().zip(&hay[s..]).all(|(q, t)| q.matches(*t)))
}

fn contains(hay: &[Folded], needle: &[Folded]) -> bool {
    occurrences(hay, needle).next().is_some()
}

/// A bookmark filter: whitespace-separated terms, matched anywhere in a title, ignoring case
/// (and accents the query leaves out).
pub struct OutlineQuery {
    terms: Vec<Vec<Folded>>,
}

impl OutlineQuery {
    pub fn new(query: &str) -> Self {
        Self {
            terms: query.split_whitespace().map(folded).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Indices of the bookmarks that match, best first. A bookmark matches when every term is
    /// in its title or an enclosing bookmark's, and at least one is in its own — so "chapter 3
    /// exercises" finds the Exercises section of Chapter 3 without listing every section of
    /// Chapter 3. Bookmarks matching on their own title come first, then those whose title
    /// and enclosing bookmarks match the most terms (for "chapter 3", Chapter 3, then its
    /// sections, then other chapters' section 3s), each group in outline order.
    pub fn filter(&self, items: &[OutlineItem]) -> Vec<usize> {
        if self.is_empty() {
            return (0..items.len()).collect();
        }
        let titles: Vec<Vec<Folded>> = items.iter().map(|it| folded(&it.title)).collect();
        // (every term in the own title, score, index)
        let mut found: Vec<(bool, usize, usize)> = Vec::new();
        'items: for i in 0..items.len() {
            let (mut own, mut inherited) = (0, 0);
            for t in &self.terms {
                let in_own = contains(&titles[i], t);
                let in_parent = std::iter::successors(items[i].parent, |&p| items[p].parent)
                    .any(|p| contains(&titles[p], t));
                if !in_own && !in_parent {
                    continue 'items;
                }
                own += usize::from(in_own);
                inherited += usize::from(in_parent);
            }
            if own > 0 {
                found.push((own == self.terms.len(), 2 * own + inherited, i));
            }
        }
        found.sort_by_key(|&(all_own, score, i)| (Reverse(all_own), Reverse(score), i));
        found.into_iter().map(|(_, _, i)| i).collect()
    }

    /// Byte ranges of `text` matching a term, merged, for highlighting.
    pub fn highlights(&self, text: &str) -> Vec<Range<usize>> {
        let starts: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        let hay = folded(text);
        let mut marked = vec![false; hay.len()];
        for t in &self.terms {
            for s in occurrences(&hay, t) {
                marked[s..s + t.len()].fill(true);
            }
        }
        let mut ranges = Vec::new();
        let mut k = 0;
        while k < marked.len() {
            if !marked[k] {
                k += 1;
                continue;
            }
            let start = starts[k];
            while k < marked.len() && marked[k] {
                k += 1;
            }
            ranges.push(start..starts.get(k).copied().unwrap_or(text.len()));
        }
        ranges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page with a match at each of these heights (page-local points).
    fn page(ys: &[f32]) -> PageHits {
        PageHits {
            rects: ys
                .iter()
                .map(|&y| Rect::from_min_size(egui::pos2(10.0, y), egui::vec2(20.0, 10.0)))
                .collect(),
            hits: (0..ys.len()).map(|i| i..i + 1).collect(),
        }
    }

    /// Search every page, in the order the search asks for them.
    fn run(search: &mut TextSearch, pages: &[&[f32]]) -> Vec<usize> {
        let mut order = Vec::new();
        while let Some(p) = search.next_page() {
            order.push(p);
            search.add_page(p, page(pages[p]));
        }
        order
    }

    #[test]
    fn starts_at_the_first_match_after_the_reading_position() {
        let pages: &[&[f32]] = &[&[100.0], &[], &[50.0, 300.0], &[20.0]];
        let mut s = TextSearch::new("x".into(), false, 4, (2, 200.0));
        assert_eq!(run(&mut s, pages), vec![2, 3, 0, 1]);
        assert_eq!(s.current, Some((2, 1)));
        assert_eq!(s.total(), 4);
        assert_eq!(s.position(), Some(3));
    }

    #[test]
    fn wraps_to_the_start_when_nothing_follows() {
        let pages: &[&[f32]] = &[&[100.0], &[], &[50.0]];
        let mut s = TextSearch::new("x".into(), false, 3, (2, 200.0));
        run(&mut s, pages);
        assert_eq!(s.current, Some((0, 0)));
    }

    #[test]
    fn steps_wrap_around_the_document() {
        let pages: &[&[f32]] = &[&[10.0, 20.0], &[], &[30.0]];
        let mut s = TextSearch::new("x".into(), false, 3, (0, 0.0));
        run(&mut s, pages);
        assert_eq!(s.current, Some((0, 0)));
        assert!(s.step(1));
        assert_eq!(s.current, Some((0, 1)));
        assert!(s.step(1));
        assert_eq!(s.current, Some((2, 0)));
        assert!(s.step(1));
        assert_eq!(s.current, Some((0, 0)));
        assert!(s.step(-1));
        assert_eq!(s.current, Some((2, 0)));
    }

    #[test]
    fn a_step_waits_for_the_pages_it_needs() {
        let pages: &[&[f32]] = &[&[10.0], &[], &[], &[40.0], &[50.0]];
        let mut s = TextSearch::new("x".into(), false, 5, (3, 0.0));
        s.add_page(3, page(pages[3]));
        assert_eq!(s.current, Some((3, 0)));
        // Backward from page 3 needs pages 2, 1, 0 — before the default order's page 4.
        assert!(!s.step(-1));
        assert_eq!(s.next_page(), Some(2));
        assert!(!s.add_page(2, page(pages[2])));
        assert_eq!(s.next_page(), Some(1));
        assert!(!s.add_page(1, page(pages[1])));
        assert!(s.add_page(0, page(pages[0])));
        assert_eq!(s.current, Some((0, 0)));
        assert_eq!(s.next_page(), Some(4));
    }

    #[test]
    fn backward_start_is_the_last_match_before_the_reading_position() {
        let pages: &[&[f32]] = &[&[10.0], &[20.0, 400.0], &[30.0]];
        let mut s = TextSearch::new("x".into(), false, 3, (1, 200.0));
        s.start_backward();
        run(&mut s, pages);
        assert_eq!(s.current, Some((1, 0)));
    }

    #[test]
    fn no_matches() {
        let pages: &[&[f32]] = &[&[], &[]];
        let mut s = TextSearch::new("x".into(), false, 2, (1, 0.0));
        run(&mut s, pages);
        assert!(s.is_complete());
        assert_eq!((s.current, s.total(), s.position()), (None, 0, None));
        assert!(!s.step(1));
    }

    fn outline(entries: &[(&str, Option<usize>)]) -> Vec<OutlineItem> {
        entries
            .iter()
            .map(|&(title, parent)| OutlineItem {
                title: title.to_owned(),
                depth: usize::from(parent.is_some()),
                parent,
                dest: None,
            })
            .collect()
    }

    #[test]
    fn bookmarks_rank_own_title_matches_first() {
        let items = outline(&[
            ("Chapter 1", None),
            ("1.3 Exercises", Some(0)),
            ("Chapter 3", None),
            ("3.1 Basics", Some(2)),
            ("3.2 Exercises", Some(2)),
        ]);
        assert_eq!(
            OutlineQuery::new("chapter 3").filter(&items),
            vec![2, 3, 4, 1]
        );
        assert_eq!(
            OutlineQuery::new("chapter 3 exercises").filter(&items),
            vec![4, 1]
        );
        assert_eq!(OutlineQuery::new("").filter(&items), vec![0, 1, 2, 3, 4]);
        assert!(OutlineQuery::new("appendix").filter(&items).is_empty());
    }

    #[test]
    fn plain_letters_match_accented_ones_but_not_the_reverse() {
        let items = outline(&[("Définition", None), ("Defined", None)]);
        assert_eq!(OutlineQuery::new("DEFIN").filter(&items), vec![0, 1]);
        assert_eq!(OutlineQuery::new("déf").filter(&items), vec![0]);
    }

    #[test]
    fn highlights_are_byte_ranges_of_the_original_title() {
        let q = OutlineQuery::new("def ion");
        let title = "Définition";
        let ranges = q.highlights(title);
        assert_eq!(ranges, vec![0..4, 8..11]);
        assert_eq!(&title[ranges[0].clone()], "Déf");
        assert_eq!(&title[ranges[1].clone()], "ion");
    }
}
