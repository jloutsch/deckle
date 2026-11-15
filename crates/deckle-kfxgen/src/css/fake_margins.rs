// SPDX-License-Identifier: GPL-3.0-only
//! `RemoveFakeMargins`: a left or right margin almost every paragraph at one depth of the book
//! shares is taken out of the classes that carry it (phase 2 plan, Task 9). Ported from
//! Calibre 9.18's `ebooks/oeb/transforms/page_margin.py` (GPL v3, `NOTICE`).
//!
//! Each element is looked at once per level, a level's statistics are a map of the values
//! seen, and the paragraphs under each `<div>` are counted in one pass over the tree, children
//! before parents.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::cascade::merged;
use super::flatten::{ClassNamer, Flat, NO_CLASS};
use super::{Decls, Parsed, parse_selector};
use crate::dom::{Dom, NodeId, XHTML_NS};

/// `RemoveFakeMargins.__call__`, over a book's spine files in spine order once each is
/// [`super::flatten`]ed with `namer`. Paragraphs (`<p>` and `<div>` in XHTML) are grouped by tag
/// and depth below `<body>` (`find_levels`): a `<p>` level of fewer than 25 is not looked
/// at, nor a `<div>` level below the second of fewer than 25, and a `<div>` of the first two
/// levels counts only with 5 paragraphs or more under it. In a level, the margins of each
/// paragraph's class (`.` and its `class` attribute, as one selector) are counted; one value
/// other than none and `0` held by more than 95% of them is removed from every class of the
/// level that has it (`analyze_stats`), so from every element of the book with that class,
/// whose styles are worked out again. A class of the level with a negative `text-indent`
/// leaves the level alone.
pub fn remove_fake_margins(namer: &mut ClassNamer, docs: &mut [(&Dom, &mut Flat)]) {
    // (tag, level) in the order first met → its paragraphs.
    let mut levels: Vec<(Level, Vec<Para>)> = Vec::new();
    let mut at: HashMap<(&'static str, usize), usize> = HashMap::new();
    // The paragraphs under each `<div>` of the first two levels.
    let mut paras_under: HashMap<(usize, NodeId), u32> = HashMap::new();
    for (i, (dom, _)) in docs.iter().enumerate() {
        // `XPath('//h:body')(root)[0]`.
        let Some(body) = dom
            .descendants()
            .find(|&e| dom.tag(e) == "body" && dom.namespace(e) == Some(XHTML_NS))
        else {
            continue;
        };
        // Body's descendants in document order, with their depth below it.
        let mut order: Vec<(NodeId, usize)> = Vec::new();
        let mut stack: Vec<(NodeId, usize)> =
            dom.child_elements(body).rev().map(|c| (c, 1)).collect();
        while let Some((e, depth)) = stack.pop() {
            order.push((e, depth));
            stack.extend(dom.child_elements(e).rev().map(|c| (c, depth + 1)));
        }
        let mut below: HashMap<NodeId, u32> = HashMap::new();
        for &(e, _) in order.iter().rev() {
            let own = below.get(&e).copied().unwrap_or(0) + u32::from(is_para(dom, e).is_some());
            if let Some(p) = dom.parent(e)
                && p != body
                && own > 0
            {
                *below.entry(p).or_insert(0) += own;
            }
        }
        for &(e, depth) in &order {
            let Some(tag) = is_para(dom, e) else {
                continue;
            };
            let key = (tag, depth);
            let slot = *at.entry(key).or_insert_with(|| {
                levels.push((key, Vec::new()));
                levels.len() - 1
            });
            levels[slot].1.push((i, e));
            if tag == "div" && depth < 3 {
                paras_under.insert((i, e), below.get(&e).copied().unwrap_or(0));
            }
        }
    }
    for ((tag, depth), elems) in &mut levels {
        let num = elems.len();
        if *tag == "p" && num < 25 {
            elems.clear();
        }
        if *tag == "div" {
            if *depth > 2 && num < 25 {
                elems.clear();
            } else if *depth < 3 {
                elems.retain(|k| paras_under.get(k).copied().unwrap_or(0) >= 5);
            }
        }
    }

    // Whether `.name` is a rule of the stylesheet (css_parser keeps it): looked up once.
    let mut is_rule: HashMap<u32, bool> = HashMap::new();
    let mut edited: HashSet<u32> = HashSet::new();
    for (_, elems) in &levels {
        if elems.is_empty() {
            continue;
        }
        // `process_level`: the margins of each paragraph's class.
        let mut found: Vec<Option<u32>> = Vec::with_capacity(elems.len());
        let mut negative = false;
        for &(i, e) in elems {
            let dom = docs[i].0;
            let id = dom
                .attr(e, "class")
                .filter(|c| !c.is_empty())
                .and_then(|c| namer.own_class(c))
                .filter(|&id| {
                    *is_rule.entry(id).or_insert_with(|| {
                        let name = &namer.classes[id as usize].name;
                        matches!(
                            parse_selector(&format!(".{name}")),
                            Parsed::Ok { pseudo: None, .. }
                        )
                    })
                });
            if let Some(id) = id
                && value(&namer.classes[id as usize].decls, "text-indent").starts_with('-')
            {
                negative = true;
                break;
            }
            found.push(id);
        }
        if negative {
            continue;
        }
        let margin = |id: Option<u32>, side: &str, namer: &ClassNamer| -> String {
            id.map_or_else(String::new, |id| {
                value(&namer.classes[id as usize].decls, side).to_owned()
            })
        };
        let remove: Vec<(&str, Option<String>)> = ["margin-left", "margin-right"]
            .into_iter()
            .map(|side| {
                let values: Vec<String> = found.iter().map(|&id| margin(id, side, namer)).collect();
                (side, fake(&values))
            })
            .collect();
        if remove.iter().all(|(_, v)| v.is_none()) {
            continue;
        }
        for &id in found.iter().flatten() {
            let (lm, rm) = (
                margin(Some(id), "margin-left", namer),
                margin(Some(id), "margin-right", namer),
            );
            for ((side, mc), m) in remove.iter().zip([lm, rm]) {
                if mc.as_deref() == Some(m.as_str()) {
                    let decls = Arc::make_mut(&mut namer.classes[id as usize].decls);
                    decls.retain(|(k, _)| k != side);
                    edited.insert(id);
                }
            }
        }
    }
    if edited.is_empty() {
        return;
    }

    // Every element of the book whose class's name is that of a class that lost a margin: the
    // declarations of every element class of that name (each a rule of `stylesheet.css` its
    // `class` attribute selects, in the sheet's order) over `html.css`'s again, as kfxgen's
    // Stylizer reads them.
    let mut touched: HashSet<u32> = HashSet::new();
    for &id in &edited {
        touched.extend(namer.own_classes(&namer.classes[id as usize].name));
    }
    for (_, flat) in docs.iter_mut() {
        let mut memo: HashMap<(usize, &str), Arc<Decls>> = HashMap::new();
        for i in 0..flat.main.len() {
            let id = flat.main[i];
            if id == NO_CLASS || !touched.contains(&id) {
                continue;
            }
            let ua = &flat.ua[i];
            let name = namer.classes[id as usize].name.as_str();
            let got = memo
                .entry((Arc::as_ptr(ua) as usize, name))
                .or_insert_with(|| {
                    let mut decls = Decls::clone(ua);
                    for same in namer.own_classes(name) {
                        decls = merged(&decls, &namer.classes[same as usize].decls);
                    }
                    Arc::new(decls)
                });
            flat.specified[i] = Arc::clone(got);
        }
    }
}

/// A level: a paragraph's tag and its depth below `<body>` (`p_1`).
type Level = (&'static str, usize);

/// A paragraph: its file's place in the spine, and the element.
type Para = (usize, NodeId);

/// `p` or `div` for a paragraph `RemoveFakeMargins` counts (`descendant::h:p|descendant::h:div`).
fn is_para(dom: &Dom, e: NodeId) -> Option<&'static str> {
    if dom.namespace(e) != Some(XHTML_NS) {
        return None;
    }
    match dom.tag(e) {
        "p" => Some("p"),
        "div" => Some("div"),
        _ => None,
    }
}

/// css_parser's `getPropertyValue`: the value, or `''`.
fn value<'a>(decls: &'a Decls, name: &str) -> &'a str {
    decls
        .iter()
        .find(|(k, _)| k == name)
        .map_or("", |(_, d)| d.value.as_str())
}

/// `analyze_stats` over a level's margins: the most common value (the first met of those
/// tied), when it is neither empty nor `0` and more than 95% of the level has it.
fn fake(values: &[String]) -> Option<String> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    let mut slot: HashMap<&str, usize> = HashMap::new();
    for v in values {
        let s = *slot.entry(v.as_str()).or_insert_with(|| {
            counts.push((v.as_str(), 0));
            counts.len() - 1
        });
        counts[s].1 += 1;
    }
    let mut best: Option<(&str, usize)> = None;
    for &(v, c) in &counts {
        if best.is_none_or(|(_, bc)| c > bc) {
            best = Some((v, c));
        }
    }
    let (most, count) = best?;
    if most.is_empty() || most == "0" {
        return None;
    }
    (count as f64 / values.len() as f64 > 0.95).then(|| most.to_owned())
}
