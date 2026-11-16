//! Charts (1.0.0 §1-3): a DrawingML chart part (`c:chartSpace`) is read from the values stored in it
//! (`c:numCache` / `c:strCache`) and drawn again as SVG — columns, bars, lines, areas, pies and
//! doughnuts; every other kind is shown as its data. Title, axis labels, legend and value labels are
//! real text, so they reach the text layer of the converted PDF (§6-6).

use std::fmt::Write as _;

use super::html::escape;
use super::model::{Block, Cell, Chart, ChartPlot, ChartSeries, Paragraph, Stacking, Table};
use super::pptx::Colors;
use super::xml::Element;

/// Guards against absurd caches.
const MAX_POINTS: usize = 2_000;
const MAX_SERIES: usize = 60;
/// Series colours when the theme names no accents.
const PALETTE: [&str; 6] = ["#4472C4", "#ED7D31", "#A5A5A5", "#FFC000", "#5B9BD5", "#70AD47"];

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Reads a chart part. `None` when it holds no series this reader knows (the caller keeps a frame).
pub fn read(space: &Element, colors: &Colors) -> Option<Chart> {
    let chart = space.child("chart")?;
    let plot_area = chart.child("plotArea")?;
    let mut out = Chart { gap_width: 150.0, hole_size: 50.0, ..Default::default() };
    let mut value_format: Option<String> = None;
    let mut first_group = true;
    for group in plot_area.elements() {
        let plot = match group.name.as_str() {
            "barChart" | "bar3DChart" => {
                if group.child("barDir").and_then(|d| d.attr("val")) == Some("bar") {
                    ChartPlot::Bar
                } else {
                    ChartPlot::Column
                }
            }
            "lineChart" | "line3DChart" | "stockChart" => ChartPlot::Line,
            "areaChart" | "area3DChart" => ChartPlot::Area,
            "pieChart" | "pie3DChart" | "ofPieChart" => ChartPlot::Pie,
            "doughnutChart" => ChartPlot::Doughnut,
            "scatterChart" | "bubbleChart" | "radarChart" | "surfaceChart" | "surface3DChart" => ChartPlot::Other,
            _ => continue,
        };
        if first_group {
            first_group = false;
            out.stacking = match group.child("grouping").and_then(|g| g.attr("val")) {
                Some("stacked") => Stacking::Stacked,
                Some("percentStacked") => Stacking::Percent,
                _ => Stacking::None,
            };
            let number = |name: &str| group.child(name).and_then(|e| e.attr_f32("val")).filter(|v| v.is_finite());
            out.gap_width = number("gapWidth").unwrap_or(150.0).clamp(0.0, 500.0);
            out.hole_size = number("holeSize").unwrap_or(50.0).clamp(10.0, 90.0);
            out.first_slice_deg = number("firstSliceAng").unwrap_or(0.0).rem_euclid(360.0);
        }
        point_labels(group.child("dLbls"), &mut out);
        for ser in group.children_named("ser") {
            if out.series.len() >= MAX_SERIES {
                break;
            }
            let index = out.series.len();
            // Scatter / bubble series name their axes differently.
            let (cats, vals) = match (ser.child("val"), ser.child("yVal")) {
                (Some(v), _) => (ser.child("cat"), v),
                (None, Some(y)) => (ser.child("xVal"), y),
                (None, None) => continue,
            };
            let (values, format) = numbers(vals);
            if value_format.is_none() {
                value_format = format;
            }
            if out.categories.is_empty() {
                if let Some(c) = cats {
                    out.categories = labels(c);
                }
            }
            point_labels(ser.child("dLbls"), &mut out);
            let accent = |i: usize| colors.accent(i).unwrap_or_else(|| PALETTE[i % PALETTE.len()].to_string());
            let pr = ser.child("spPr");
            // A line is coloured by its stroke, everything else by its fill.
            let own = match plot {
                ChartPlot::Line => pr.and_then(|p| colors.line(p)).map(|(c, _)| c),
                _ => pr.and_then(|p| colors.fill(p)).flatten(),
            };
            if matches!(plot, ChartPlot::Pie | ChartPlot::Doughnut) && out.point_colors.is_empty() {
                out.point_colors = (0..values.len())
                    .map(|i| {
                        ser.children_named("dPt")
                            .find(|d| d.child("idx").and_then(|x| x.attr_u32("val")) == Some(i as u32))
                            .and_then(|d| d.child("spPr"))
                            .and_then(|p| colors.fill(p))
                            .flatten()
                            .unwrap_or_else(|| accent(i))
                    })
                    .collect();
            }
            out.series.push(ChartSeries {
                name: ser.child("tx").and_then(text_of).unwrap_or_else(|| format!("계열 {}", index + 1)),
                values,
                color: own.unwrap_or_else(|| accent(index)),
                plot,
            });
        }
    }
    if out.series.is_empty() {
        return None;
    }
    // Shared axes hold columns, lines and areas together; bars only bars; a pie is drawn from its
    // first series. Anything else is shown as its data.
    let upright = |p: ChartPlot| matches!(p, ChartPlot::Column | ChartPlot::Line | ChartPlot::Area);
    out.plot = match out.series[0].plot {
        p if upright(p) && out.series.iter().all(|s| upright(s.plot)) => p,
        ChartPlot::Bar if out.series.iter().all(|s| s.plot == ChartPlot::Bar) => ChartPlot::Bar,
        p @ (ChartPlot::Pie | ChartPlot::Doughnut) => p,
        _ => ChartPlot::Other,
    };
    // One label and one value slot per point.
    let points = out.series.iter().map(|s| s.values.len()).max().unwrap_or(0).max(out.categories.len());
    for i in out.categories.len()..points {
        out.categories.push((i + 1).to_string());
    }
    for s in &mut out.series {
        s.values.resize(points, None);
    }

    let mut axis_format = None;
    for axis in plot_area.elements() {
        match axis.name.as_str() {
            "catAx" | "dateAx" => {
                out.category_title = axis.child("title").and_then(title_text);
                out.reversed = axis.child("scaling").and_then(|s| s.child("orientation")).and_then(|o| o.attr("val"))
                    == Some("maxMin");
            }
            "valAx" if out.value_title.is_none() && axis_format.is_none() => {
                out.value_title = axis.child("title").and_then(title_text);
                // A format "linked to source" is the one of the data.
                axis_format = axis
                    .child("numFmt")
                    .filter(|f| f.attr_bool("sourceLinked") != Some(true))
                    .and_then(|f| f.attr("formatCode"))
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    out.number_format = axis_format.or(value_format).filter(|f| !f.trim().eq_ignore_ascii_case("general"));
    // Without a title of its own a chart of one series is titled by that series.
    let auto_title = |out: &Chart| (out.series.len() == 1).then(|| out.series[0].name.clone());
    out.title = match chart.child("title") {
        Some(t) => title_text(t).or_else(|| auto_title(&out)),
        None if chart.child("autoTitleDeleted").and_then(|a| a.attr_bool("val")) == Some(true) => None,
        None => auto_title(&out),
    };
    // `sz` is in hundredths of a point.
    out.font_pt = space
        .child("txPr")
        .and_then(|t| t.find("defRPr"))
        .and_then(|d| d.attr_f32("sz"))
        .filter(|sz| sz.is_finite() && *sz > 0.0)
        .map(|sz| sz / 100.0);
    out.legend = chart.child("legend").map(|l| match l.child("legendPos").and_then(|p| p.attr("val")) {
        Some("b") => 'b',
        Some("t") => 't',
        Some("l") => 'l',
        _ => 'r',
    });
    Some(out)
}

/// `c:dLbls`: which labels the points carry.
fn point_labels(labels: Option<&Element>, out: &mut Chart) {
    let Some(labels) = labels else { return };
    if labels.child("delete").and_then(|d| d.attr_bool("val")) == Some(true) {
        return;
    }
    let on = |name: &str| labels.child(name).and_then(|e| e.attr_bool("val")) == Some(true);
    out.show_values |= on("showVal");
    out.show_percent |= on("showPercent");
}

/// The text of a `c:tx` (a series name): a cached reference, rich text or a literal.
fn text_of(tx: &Element) -> Option<String> {
    let text = if let Some(cache) = tx.find("strCache") {
        cache.find("v").map(|v| v.text())
    } else if let Some(rich) = tx.child("rich") {
        Some(rich_text(rich))
    } else {
        tx.child("v").map(|v| v.text())
    };
    text.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The text of a `c:title` that carries its own (`c:tx/c:rich`).
fn title_text(title: &Element) -> Option<String> {
    let text = rich_text(title.child("tx")?.child("rich")?);
    (!text.is_empty()).then_some(text)
}

/// Paragraphs of a rich text body, joined by a space.
fn rich_text(rich: &Element) -> String {
    let mut paragraphs = Vec::new();
    rich.find_all("p", &mut paragraphs);
    paragraphs
        .iter()
        .map(|p| {
            let mut runs = Vec::new();
            p.find_all("t", &mut runs);
            runs.iter().map(|t| t.text()).collect::<String>().trim().to_string()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The points of a cache (`c:strCache`, `c:numCache`, a literal, one level of a multi-level cache) by index.
fn points(cache: &Element, declared: Option<u32>) -> Vec<Option<String>> {
    let declared = declared.or_else(|| cache.child("ptCount").and_then(|c| c.attr_u32("val"))).unwrap_or(0) as usize;
    let mut out: Vec<Option<String>> = vec![None; declared.min(MAX_POINTS)];
    for pt in cache.children_named("pt") {
        let Some(i) = pt.attr_u32("idx").map(|i| i as usize).filter(|i| *i < MAX_POINTS) else { continue };
        if i >= out.len() {
            out.resize(i + 1, None);
        }
        out[i] = pt.child("v").map(|v| v.text());
    }
    out
}

/// Values of a `c:val` / `c:yVal` and the number format they were stored with.
fn numbers(val: &Element) -> (Vec<Option<f64>>, Option<String>) {
    let Some(cache) = val.find("numCache").or_else(|| val.find("numLit")) else { return (Vec::new(), None) };
    let format = cache.child("formatCode").map(|f| f.text().trim().to_string()).filter(|f| !f.is_empty());
    let values = points(cache, None)
        .into_iter()
        .map(|p| p.and_then(|s| s.trim().parse::<f64>().ok()).filter(|v| v.is_finite()))
        .collect();
    (values, format)
}

/// Labels of a `c:cat` / `c:xVal`: strings, the innermost level of a multi-level axis, or numbers
/// (dates when the format says so).
fn labels(cat: &Element) -> Vec<String> {
    let strings = |ps: Vec<Option<String>>| ps.into_iter().map(|p| p.unwrap_or_default().trim().to_string()).collect();
    if let Some(cache) = cat.find("strCache").or_else(|| cat.find("strLit")) {
        return strings(points(cache, None));
    }
    if let Some(multi) = cat.find("multiLvlStrCache") {
        let declared = multi.child("ptCount").and_then(|c| c.attr_u32("val"));
        return multi.child("lvl").map(|lvl| strings(points(lvl, declared))).unwrap_or_default();
    }
    if let Some(cache) = cat.find("numCache").or_else(|| cat.find("numLit")) {
        let format = cache.child("formatCode").map(|f| f.text());
        return points(cache, None)
            .into_iter()
            .map(|p| match p.and_then(|s| s.trim().parse::<f64>().ok()) {
                Some(v) => date_label(v, format.as_deref()).unwrap_or_else(|| format_number(v, format.as_deref())),
                None => String::new(),
            })
            .collect();
    }
    Vec::new()
}

/// A serial day number as a date when `format` is a date format (`yyyy-mm-dd`, `mmm-yy` …).
fn date_label(serial: f64, format: Option<&str>) -> Option<String> {
    let f = format?.to_ascii_lowercase();
    if !(f.contains('y') || f.contains("mmm") || (f.contains('m') && f.contains('d'))) || f.contains('0') {
        return None;
    }
    if !(1.0..=2_958_465.0).contains(&serial) {
        return None;
    }
    let date =
        chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?.checked_add_signed(chrono::Duration::days(serial as i64))?;
    Some(date.format(if f.contains('d') { "%Y-%m-%d" } else { "%Y-%m" }).to_string())
}

/// A number in a spreadsheet format as far as charts use them: percent, thousands separators, fixed
/// decimals, a currency sign. `None` / `General`: as short as the value allows.
pub fn format_number(v: f64, format: Option<&str>) -> String {
    let section = format.unwrap_or("General").split(';').next().unwrap_or("General");
    let general = !section.contains(['0', '#']);
    let percent = section.contains('%');
    let v = if percent { v * 100.0 } else { v };
    let decimals = if general {
        if (v - v.round()).abs() < 1e-9 {
            0
        } else if v.abs() < 0.01 {
            4
        } else {
            2
        }
    } else {
        section.split_once('.').map(|(_, d)| d.chars().take_while(|c| *c == '0').count()).unwrap_or(0)
    };
    let mut s = format!("{v:.decimals$}");
    if general && s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if s.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') {
        s = s.trim_start_matches('-').to_string();
    }
    if section.contains(",0") || section.contains(",#") {
        s = group_thousands(&s);
    }
    if let Some(sign) = ['₩', '$', '€', '¥', '£'].into_iter().find(|c| section.contains(*c)) {
        s = match s.strip_prefix('-') {
            Some(rest) => format!("-{sign}{rest}"),
            None => format!("{sign}{s}"),
        };
    }
    if percent {
        s.push('%');
    }
    s
}

fn group_thousands(s: &str) -> String {
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", s),
    };
    let (int, frac) = rest.split_once('.').map(|(i, f)| (i, Some(f))).unwrap_or((rest, None));
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// The data of a chart as a table: categories down, series across. What a chart that is not drawn
/// looks like, and what the reading-order text of every chart carries.
pub fn data_table(chart: &Chart) -> Table {
    let cell = |text: String, header: bool| Cell {
        blocks: vec![Block::Paragraph(Paragraph::plain(text))],
        row_span: 1,
        col_span: 1,
        header,
        style: None,
    };
    let mut rows = Vec::with_capacity(chart.categories.len() + 1);
    let mut head = vec![cell(chart.category_title.clone().unwrap_or_default(), true)];
    head.extend(chart.series.iter().map(|s| cell(s.name.clone(), true)));
    rows.push(head);
    for (i, category) in chart.categories.iter().enumerate() {
        let mut row = vec![cell(category.clone(), false)];
        row.extend(chart.series.iter().map(|s| {
            let v = s.values.get(i).copied().flatten();
            cell(v.map(|v| format_number(v, chart.number_format.as_deref())).unwrap_or_default(), false)
        }));
        rows.push(row);
    }
    Table { rows, col_widths_mm: Vec::new(), row_heights_mm: Vec::new() }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Only colours this module (or the reader) wrote reach the markup.
fn color(c: &str) -> &str {
    if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()) {
        c
    } else {
        "#808080"
    }
}

fn is_dark(c: &str) -> bool {
    let c = color(c);
    let part = |i: usize| u32::from_str_radix(&c[i..i + 2], 16).unwrap_or(128) as f32;
    0.299 * part(1) + 0.587 * part(3) + 0.114 * part(5) < 150.0
}

/// A rough width of `s` at font size `fs` (Hangul and CJK glyphs are a full em, the rest a little over half).
fn text_width(s: &str, fs: f32) -> f32 {
    s.chars().map(|c| if c as u32 >= 0x1100 { 1.0 } else { 0.56 }).sum::<f32>() * fs
}

/// `s`, cut with an ellipsis when it is wider than `max`.
fn fit(s: &str, max: f32, fs: f32) -> String {
    if text_width(s, fs) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        let mut next = out.clone();
        next.push(ch);
        if text_width(&next, fs) + fs * 0.9 > max {
            break;
        }
        out = next;
    }
    out.push('…');
    out
}

fn text(s: &mut String, x: f32, y: f32, anchor: &str, size: f32, extra: &str, content: &str) {
    let _ = write!(
        s,
        "<text x=\"{x:.1}\" y=\"{y:.1}\" text-anchor=\"{anchor}\" font-size=\"{size:.1}\"{extra}>{}</text>",
        escape(content)
    );
}

fn line(s: &mut String, x1: f32, y1: f32, x2: f32, y2: f32, stroke: &str, width: f32) {
    let _ = write!(
        s,
        "<line x1=\"{x1:.1}\" y1=\"{y1:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\" stroke=\"{stroke}\" stroke-width=\"{width}\"/>"
    );
}

/// A value axis that holds `min` … `max` and zero: (low, high, step) in about five round steps.
fn axis(min: f64, max: f64) -> (f64, f64, f64) {
    let (lo, hi) = (min.min(0.0), max.max(0.0));
    let span = if hi - lo > 0.0 { hi - lo } else { 1.0 };
    let raw = span / 5.0;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].into_iter().map(|m| m * magnitude).find(|s| *s >= raw * (1.0 - 1e-9));
    let step = step.unwrap_or(10.0 * magnitude);
    let (low, mut high) = ((lo / step).floor() * step, (hi / step).ceil() * step);
    if high <= low {
        high = low + step * 5.0;
    }
    (low, high, step)
}

/// Where each series reaches at each category: `(from, to)` — from zero, or from the running total
/// of the series before it when it stacks. Returns the spans and the smallest / largest value touched.
type Spans = Vec<Vec<Option<(f64, f64)>>>;

fn spans(chart: &Chart, stacks: impl Fn(ChartPlot) -> bool) -> (Spans, f64, f64) {
    let n = chart.categories.len();
    let percent = chart.stacking == Stacking::Percent;
    let value = |s: &ChartSeries, i: usize| s.values.get(i).copied().flatten();
    let totals: Vec<f64> = (0..n)
        .map(|i| chart.series.iter().filter(|s| stacks(s.plot)).filter_map(|s| value(s, i)).map(f64::abs).sum())
        .collect();
    let (mut up, mut down) = (vec![0.0f64; n], vec![0.0f64; n]);
    let (mut min, mut max) = (0.0f64, 0.0f64);
    let all = chart
        .series
        .iter()
        .map(|s| {
            (0..n)
                .map(|i| {
                    let v = value(s, i)?;
                    let span = if stacks(s.plot) {
                        let v = if percent {
                            if totals[i] > 0.0 {
                                v / totals[i]
                            } else {
                                0.0
                            }
                        } else {
                            v
                        };
                        let total = if v >= 0.0 { &mut up[i] } else { &mut down[i] };
                        let from = *total;
                        *total += v;
                        (from, *total)
                    } else {
                        (0.0, v)
                    };
                    min = min.min(span.0).min(span.1);
                    max = max.max(span.0).max(span.1);
                    Some(span)
                })
                .collect()
        })
        .collect();
    (all, min, max)
}

/// The chart as an SVG that fills a box of `w_mm` × `h_mm` (user units are CSS pixels).
pub fn svg(chart: &Chart, w_mm: f32, h_mm: f32) -> String {
    let (w, h) = ((w_mm * 96.0 / 25.4).max(60.0), (h_mm * 96.0 / 25.4).max(40.0));
    // The chart's own text size when it sets one — as far as the box can hold it.
    let fs = match chart.font_pt.filter(|pt| pt.is_finite()) {
        Some(pt) => (pt * 96.0 / 72.0).clamp(7.0, (h * 0.065).max(7.0)),
        None => (h * 0.042).clamp(7.0, 13.0),
    };
    let mut s = format!("<svg class=\"chart\" viewBox=\"0 0 {w:.1} {h:.1}\" fill=\"#404040\">");
    // The room that is left as the title, the legend and the axis labels take theirs.
    let mut area = Rect { x: 6.0, y: 6.0, w: w - 12.0, h: h - 12.0 };
    if let Some(title) = chart.title.as_deref().filter(|t| !t.trim().is_empty()) {
        let size = fs * 1.45;
        text(&mut s, w / 2.0, area.y + size, "middle", size, " font-weight=\"600\"", &fit(title, area.w, size));
        area.y += size * 1.6;
        area.h -= size * 1.6;
    }
    let pie = matches!(chart.plot, ChartPlot::Pie | ChartPlot::Doughnut);
    if let Some(position) = chart.legend {
        // A pie names its slices, every other chart its series.
        let entries: Vec<(&str, &str)> = if pie {
            chart
                .categories
                .iter()
                .enumerate()
                .map(|(i, c)| (c.as_str(), chart.point_colors.get(i).map(String::as_str).unwrap_or(PALETTE[i % 6])))
                .collect()
        } else {
            chart.series.iter().map(|s| (s.name.as_str(), s.color.as_str())).collect()
        };
        legend(&mut s, &mut area, position, &entries, fs);
    }
    if area.w > 20.0 && area.h > 20.0 && !chart.categories.is_empty() {
        match chart.plot {
            ChartPlot::Pie | ChartPlot::Doughnut => slices(&mut s, chart, area, fs),
            ChartPlot::Bar => bars(&mut s, chart, area, fs),
            _ => upright(&mut s, chart, area, fs),
        }
    }
    s.push_str("</svg>");
    s
}

fn legend(s: &mut String, area: &mut Rect, position: char, entries: &[(&str, &str)], fs: f32) {
    if entries.is_empty() {
        return;
    }
    let mark = fs * 0.8;
    let row_h = fs * 1.5;
    let entry = |s: &mut String, x: f32, y: f32, label: &str, fill: &str, room: f32| {
        let _ = write!(
            s,
            "<rect x=\"{x:.1}\" y=\"{:.1}\" width=\"{mark:.1}\" height=\"{mark:.1}\" fill=\"{}\"/>",
            y - mark / 2.0,
            color(fill)
        );
        text(s, x + mark + 4.0, y + fs * 0.35, "start", fs, "", &fit(label, room, fs));
    };
    if matches!(position, 'b' | 't') {
        // Rows of entries, each row centred.
        let width = |label: &str| (mark + 4.0 + text_width(label, fs) + fs).min(area.w);
        let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
        let mut used = 0.0;
        for (i, (label, _)) in entries.iter().enumerate() {
            if used + width(label) > area.w && !rows[rows.len() - 1].is_empty() {
                rows.push(Vec::new());
                used = 0.0;
            }
            let last = rows.len() - 1;
            rows[last].push(i);
            used += width(label);
        }
        rows.truncate(3);
        let total = row_h * rows.len() as f32;
        let top = if position == 'b' { area.y + area.h - total } else { area.y };
        for (r, row) in rows.iter().enumerate() {
            let row_w: f32 = row.iter().map(|i| width(entries[*i].0)).sum();
            let mut x = area.x + (area.w - row_w).max(0.0) / 2.0;
            let y = top + r as f32 * row_h + row_h / 2.0;
            for i in row {
                let (label, fill) = entries[*i];
                entry(s, x, y, label, fill, width(label) - mark - 4.0);
                x += width(label);
            }
        }
        if position == 't' {
            area.y += total + 2.0;
        }
        area.h -= total + 2.0;
    } else {
        let widest = entries.iter().map(|(l, _)| mark + 4.0 + text_width(l, fs)).fold(0.0, f32::max);
        let column = (widest + 4.0).min(area.w * 0.35);
        let shown = ((area.h / row_h).floor() as usize).clamp(1, entries.len());
        let x = if position == 'l' { area.x } else { area.x + area.w - column };
        let top = area.y + (area.h - shown as f32 * row_h).max(0.0) / 2.0;
        for (i, (label, fill)) in entries.iter().take(shown).enumerate() {
            entry(s, x, top + i as f32 * row_h + row_h / 2.0, label, fill, column - mark - 4.0);
        }
        if position == 'l' {
            area.x += column + 6.0;
        }
        area.w -= column + 6.0;
    }
}

/// Ticks of a value axis and their labels.
fn ticks(lo: f64, hi: f64, step: f64, format: Option<&str>) -> Vec<(f64, String)> {
    let count = (((hi - lo) / step).round() as usize).min(40);
    (0..=count)
        .map(|k| {
            let v = lo + k as f64 * step;
            // Sums of a decimal step drift (0.30000000000000004).
            let v = (v / step).round() * step;
            (v, format_number(v, format))
        })
        .collect()
}

fn value_scale(chart: &Chart, min: f64, max: f64) -> (f64, f64, Vec<(f64, String)>) {
    if chart.stacking == Stacking::Percent {
        let lo = if min < 0.0 { -1.0 } else { 0.0 };
        return (lo, 1.0, ticks(lo, 1.0, 0.2, Some("0%")));
    }
    let (mut lo, mut hi, step) = axis(min, max);
    // Labels beyond the ends of the bars need room between the data and the end of the axis.
    if chart.show_values && chart.stacking == Stacking::None {
        if max > 0.0 && hi - max < step * 0.35 {
            hi += step;
        }
        if min < 0.0 && min - lo < step * 0.35 {
            lo -= step;
        }
    }
    (lo, hi, ticks(lo, hi, step, chart.number_format.as_deref()))
}

/// The label of a data point.
fn point_label(chart: &Chart, v: f64) -> String {
    let format = if chart.stacking == Stacking::Percent { Some("0%") } else { chart.number_format.as_deref() };
    format_number(v, format)
}

/// Columns, lines and areas over a horizontal category axis.
fn upright(s: &mut String, chart: &Chart, area: Rect, fs: f32) {
    let n = chart.categories.len();
    let stacked = chart.stacking != Stacking::None;
    let (spans, min, max) = spans(chart, |p| stacked && matches!(p, ChartPlot::Column | ChartPlot::Area));
    let (lo, hi, ticks) = value_scale(chart, min, max);
    let title_room = |t: &Option<String>| if t.is_some() { fs * 1.6 } else { 0.0 };
    let mut left =
        ticks.iter().map(|(_, l)| text_width(l, fs)).fold(0.0, f32::max) + 8.0 + title_room(&chart.value_title);
    let widest = chart.categories.iter().map(|c| text_width(c, fs)).fold(0.0, f32::max);
    // Labels that do not fit under their band are slanted.
    let slanted = n > 1 && widest > (area.w - left) / n as f32 - 2.0;
    let label_room = if slanted { (widest * 0.71 + fs).min(area.h * 0.35) } else { fs * 1.7 };
    if slanted {
        // A slanted label ends under its point and runs down to the left: the first one needs that room.
        let reach = (label_room - fs).max(0.0);
        left = left.max(reach - (area.w - left) / n as f32 / 2.0 + 4.0).min(area.w * 0.45);
    }
    let below = label_room + title_room(&chart.category_title);
    let plot = Rect {
        x: area.x + left,
        y: area.y + fs * 0.9,
        w: (area.w - left - fs * 0.6).max(10.0),
        h: (area.h - below - fs * 0.9).max(10.0),
    };
    let y_of = |v: f64| plot.y + plot.h - ((v - lo) / (hi - lo)) as f32 * plot.h;
    let band = plot.w / n as f32;
    let center = |i: usize| plot.x + band * (i as f32 + 0.5);
    for (v, label) in &ticks {
        let y = y_of(*v);
        line(s, plot.x, y, plot.x + plot.w, y, "#D9D9D9", 0.75);
        text(s, plot.x - 5.0, y + fs * 0.35, "end", fs, "", label);
    }
    // Areas lie at the back, then the columns, the lines on top.
    for (ser, span) in chart.series.iter().zip(&spans).filter(|(ser, _)| ser.plot == ChartPlot::Area) {
        let reach = |i: usize| span[i].unwrap_or((0.0, 0.0));
        let mut d = String::new();
        for i in 0..n {
            let _ = write!(d, "{}{:.1} {:.1}", if i == 0 { 'M' } else { 'L' }, center(i), y_of(reach(i).1));
        }
        for i in (0..n).rev() {
            let _ = write!(d, "L{:.1} {:.1}", center(i), y_of(reach(i).0));
        }
        let opacity = if stacked { 1.0 } else { 0.8 };
        let _ = write!(s, "<path d=\"{d}Z\" fill=\"{}\" fill-opacity=\"{opacity}\"/>", color(&ser.color));
    }
    let columns: Vec<usize> = (0..chart.series.len()).filter(|j| chart.series[*j].plot == ChartPlot::Column).collect();
    let slots = if stacked { 1.0 } else { columns.len().max(1) as f32 };
    let bar = band / (slots + chart.gap_width / 100.0);
    for (slot, j) in columns.iter().enumerate() {
        let ser = &chart.series[*j];
        for (i, span) in spans[*j].iter().enumerate() {
            let Some((from, to)) = *span else { continue };
            let x =
                plot.x + band * i as f32 + (band - bar * slots) / 2.0 + if stacked { 0.0 } else { slot as f32 * bar };
            let (top, bottom) = (y_of(from.max(to)), y_of(from.min(to)));
            let _ = write!(
                s,
                "<rect x=\"{x:.1}\" y=\"{top:.1}\" width=\"{bar:.1}\" height=\"{:.1}\" fill=\"{}\"/>",
                (bottom - top).max(0.0),
                color(&ser.color)
            );
            if chart.show_values {
                let label = point_label(chart, to - from);
                if stacked {
                    let fill = if is_dark(&ser.color) { " fill=\"#FFFFFF\"" } else { "" };
                    if bottom - top >= fs * 1.1 {
                        text(s, x + bar / 2.0, (top + bottom) / 2.0 + fs * 0.32, "middle", fs * 0.9, fill, &label);
                    }
                } else {
                    let y = if to >= from { top - fs * 0.35 } else { bottom + fs };
                    text(s, x + bar / 2.0, y, "middle", fs * 0.9, "", &label);
                }
            }
        }
    }
    for (ser, span) in chart.series.iter().zip(&spans).filter(|(ser, _)| ser.plot == ChartPlot::Line) {
        let stroke = color(&ser.color);
        let (mut d, mut pen_down) = (String::new(), false);
        let mut marks = String::new();
        for (i, point) in span.iter().enumerate() {
            let Some((_, v)) = *point else {
                pen_down = false;
                continue;
            };
            let (x, y) = (center(i), y_of(v));
            let _ = write!(d, "{}{x:.1} {y:.1}", if pen_down { 'L' } else { 'M' });
            pen_down = true;
            let _ = write!(marks, "<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{:.1}\" fill=\"{stroke}\"/>", fs * 0.22);
            if chart.show_values {
                text(&mut marks, x, y - fs * 0.6, "middle", fs * 0.9, "", &point_label(chart, v));
            }
        }
        let _ =
            write!(s, "<path d=\"{d}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{:.1}\"/>{marks}", fs * 0.18);
    }
    line(s, plot.x, y_of(0.0), plot.x + plot.w, y_of(0.0), "#8C8C8C", 1.0);
    // Category labels; a crowded axis names every k-th point.
    let needed = if slanted { fs * 1.5 } else { 1.0 };
    let every = (needed / band).ceil().max(1.0) as usize;
    let y = plot.y + plot.h + fs * 1.25;
    for (i, label) in chart.categories.iter().enumerate().step_by(every) {
        if slanted {
            let label = fit(label, (label_room - fs) / 0.71, fs);
            let extra = format!(" transform=\"rotate(-45 {:.1} {:.1})\"", center(i), y - fs * 0.4);
            text(s, center(i), y - fs * 0.4, "end", fs, &extra, &label);
        } else {
            text(s, center(i), y, "middle", fs, "", &fit(label, band * every as f32 - 2.0, fs));
        }
    }
    if let Some(title) = &chart.category_title {
        text(s, plot.x + plot.w / 2.0, area.y + area.h - fs * 0.3, "middle", fs, "", &fit(title, plot.w, fs));
    }
    if let Some(title) = &chart.value_title {
        let (x, y) = (area.x + fs, plot.y + plot.h / 2.0);
        let extra = format!(" transform=\"rotate(-90 {x:.1} {y:.1})\"");
        text(s, x, y, "middle", fs, &extra, &fit(title, plot.h, fs));
    }
}

/// Horizontal bars: categories up the left side (the first one at the bottom unless the axis is
/// reversed), values along the bottom.
fn bars(s: &mut String, chart: &Chart, area: Rect, fs: f32) {
    let n = chart.categories.len();
    let stacked = chart.stacking != Stacking::None;
    let (spans, min, max) = spans(chart, |_| stacked);
    let (lo, hi, ticks) = value_scale(chart, min, max);
    let title_room = |t: &Option<String>| if t.is_some() { fs * 1.6 } else { 0.0 };
    let widest = chart.categories.iter().map(|c| text_width(c, fs)).fold(0.0, f32::max);
    let label_room = (widest + 8.0).min(area.w * 0.4);
    let left = label_room + title_room(&chart.category_title);
    let below = fs * 1.7 + title_room(&chart.value_title);
    let last = ticks.last().map(|(_, l)| text_width(l, fs) / 2.0).unwrap_or(0.0);
    let plot = Rect {
        x: area.x + left,
        y: area.y + fs * 0.4,
        w: (area.w - left - last - 4.0).max(10.0),
        h: (area.h - below - fs * 0.4).max(10.0),
    };
    let x_of = |v: f64| plot.x + ((v - lo) / (hi - lo)) as f32 * plot.w;
    let band = plot.h / n as f32;
    // Row 0 is the top one.
    let row = |i: usize| if chart.reversed { i } else { n - 1 - i };
    for (v, label) in &ticks {
        let x = x_of(*v);
        line(s, x, plot.y, x, plot.y + plot.h, "#D9D9D9", 0.75);
        text(s, x, plot.y + plot.h + fs * 1.25, "middle", fs, "", label);
    }
    let slots = if stacked { 1.0 } else { chart.series.len().max(1) as f32 };
    let bar = band / (slots + chart.gap_width / 100.0);
    for (j, ser) in chart.series.iter().enumerate() {
        // Within a group the first series is next to the axis' start, like the categories.
        let slot = if stacked {
            0.0
        } else if chart.reversed {
            j as f32
        } else {
            slots - 1.0 - j as f32
        };
        for (i, span) in spans[j].iter().enumerate() {
            let Some((from, to)) = *span else { continue };
            let y = plot.y + band * row(i) as f32 + (band - bar * slots) / 2.0 + slot * bar;
            let (x0, x1) = (x_of(from.min(to)), x_of(from.max(to)));
            let _ = write!(
                s,
                "<rect x=\"{x0:.1}\" y=\"{y:.1}\" width=\"{:.1}\" height=\"{bar:.1}\" fill=\"{}\"/>",
                (x1 - x0).max(0.0),
                color(&ser.color)
            );
            if chart.show_values {
                let label = point_label(chart, to - from);
                let mid = y + bar / 2.0 + fs * 0.32;
                if stacked {
                    let fill = if is_dark(&ser.color) { " fill=\"#FFFFFF\"" } else { "" };
                    if x1 - x0 >= text_width(&label, fs * 0.9) + 2.0 {
                        text(s, (x0 + x1) / 2.0, mid, "middle", fs * 0.9, fill, &label);
                    }
                } else if to >= from {
                    text(s, x1 + 3.0, mid, "start", fs * 0.9, "", &label);
                } else {
                    text(s, x0 - 3.0, mid, "end", fs * 0.9, "", &label);
                }
            }
        }
    }
    line(s, x_of(0.0), plot.y, x_of(0.0), plot.y + plot.h, "#8C8C8C", 1.0);
    let every = (fs * 1.1 / band).ceil().max(1.0) as usize;
    for (i, label) in chart.categories.iter().enumerate().step_by(every) {
        let y = plot.y + band * (row(i) as f32 + 0.5) + fs * 0.35;
        text(s, plot.x - 5.0, y, "end", fs, "", &fit(label, label_room - 6.0, fs));
    }
    if let Some(title) = &chart.value_title {
        text(s, plot.x + plot.w / 2.0, area.y + area.h - fs * 0.3, "middle", fs, "", &fit(title, plot.w, fs));
    }
    if let Some(title) = &chart.category_title {
        let (x, y) = (area.x + fs, plot.y + plot.h / 2.0);
        let extra = format!(" transform=\"rotate(-90 {x:.1} {y:.1})\"");
        text(s, x, y, "middle", fs, &extra, &fit(title, plot.h, fs));
    }
}

/// A pie or a doughnut of the first series: slices clockwise from the top.
fn slices(s: &mut String, chart: &Chart, area: Rect, fs: f32) {
    let values: Vec<f64> = chart.series[0].values.iter().map(|v| v.unwrap_or(0.0).max(0.0)).collect();
    let total: f64 = values.iter().sum();
    if total <= 0.0 {
        return;
    }
    let (cx, cy) = (area.x + area.w / 2.0, area.y + area.h / 2.0);
    let r = (area.w.min(area.h) / 2.0 - fs * 0.4).max(5.0);
    let hole = if chart.plot == ChartPlot::Doughnut { r * chart.hole_size.clamp(10.0, 90.0) / 100.0 } else { 0.0 };
    let at = |radius: f32, degrees: f64| {
        let a = degrees.to_radians();
        (cx + radius * a.sin() as f32, cy - radius * a.cos() as f32)
    };
    let mut angle = chart.first_slice_deg as f64;
    for (i, v) in values.iter().enumerate() {
        if *v <= 0.0 {
            continue;
        }
        let sweep = v / total * 360.0;
        let fill = color(chart.point_colors.get(i).map(String::as_str).unwrap_or(PALETTE[i % 6]));
        if sweep >= 359.99 {
            // One slice is the whole: a ring (or a disc) rather than an arc back onto itself.
            let _ = write!(
                s,
                "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{:.1}\" fill=\"none\" stroke=\"{fill}\" stroke-width=\"{:.1}\"/>",
                (r + hole) / 2.0,
                r - hole
            );
        } else {
            let large = u8::from(sweep > 180.0);
            let (a, b) = (at(r, angle), at(r, angle + sweep));
            let mut d = format!("M{:.1} {:.1}A{r:.1} {r:.1} 0 {large} 1 {:.1} {:.1}", a.0, a.1, b.0, b.1);
            if hole > 0.0 {
                let (c, e) = (at(hole, angle + sweep), at(hole, angle));
                let _ = write!(d, "L{:.1} {:.1}A{hole:.1} {hole:.1} 0 {large} 0 {:.1} {:.1}Z", c.0, c.1, e.0, e.1);
            } else {
                let _ = write!(d, "L{cx:.1} {cy:.1}Z");
            }
            let _ = write!(s, "<path d=\"{d}\" fill=\"{fill}\" stroke=\"#FFFFFF\" stroke-width=\"1\"/>");
        }
        if (chart.show_percent || chart.show_values) && sweep >= 14.0 {
            let label = if chart.show_percent {
                format!("{:.0}%", v / total * 100.0)
            } else {
                format_number(*v, chart.number_format.as_deref())
            };
            let radius = if hole > 0.0 { (r + hole) / 2.0 } else { r * 0.65 };
            let (x, y) = at(radius, angle + sweep / 2.0);
            let extra = if is_dark(fill) { " fill=\"#FFFFFF\"" } else { "" };
            text(s, x, y + fs * 0.35, "middle", fs, extra, &label);
        }
        angle += sweep;
    }
}

/// A combined chart for tests: two column series (one point missing, one coloured by hand) and a line
/// on shared axes, with a title, axis titles, value labels and a legend below.
#[cfg(test)]
pub(crate) const SAMPLE_CHART: &str = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>분기별 </a:t></a:r><a:r><a:t>실적</a:t></a:r></a:p></c:rich></c:tx></c:title><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:ser><c:idx val="0"/><c:tx><c:strRef><c:f>Sheet1!$B$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>매출</c:v></c:pt></c:strCache></c:strRef></c:tx><c:cat><c:strRef><c:strCache><c:ptCount val="3"/><c:pt idx="0"><c:v>1분기</c:v></c:pt><c:pt idx="1"><c:v>2분기</c:v></c:pt><c:pt idx="2"><c:v>3분기</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:numCache><c:formatCode>#,##0</c:formatCode><c:ptCount val="3"/><c:pt idx="0"><c:v>1200</c:v></c:pt><c:pt idx="2"><c:v>1380</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser><c:ser><c:idx val="1"/><c:tx><c:v>비용</c:v></c:tx><c:spPr><a:solidFill><a:srgbClr val="C00000"/></a:solidFill></c:spPr><c:val><c:numRef><c:numCache><c:formatCode>#,##0</c:formatCode><c:ptCount val="3"/><c:pt idx="0"><c:v>800</c:v></c:pt><c:pt idx="1"><c:v>910</c:v></c:pt><c:pt idx="2"><c:v>870</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser><c:dLbls><c:showVal val="1"/></c:dLbls><c:gapWidth val="100"/></c:barChart><c:lineChart><c:grouping val="standard"/><c:ser><c:idx val="2"/><c:tx><c:v>목표</c:v></c:tx><c:spPr><a:ln w="28575"><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></a:ln></c:spPr><c:val><c:numLit><c:ptCount val="3"/><c:pt idx="0"><c:v>1000</c:v></c:pt><c:pt idx="1"><c:v>1100</c:v></c:pt><c:pt idx="2"><c:v>1250</c:v></c:pt></c:numLit></c:val></c:ser></c:lineChart><c:catAx><c:title><c:tx><c:rich><a:p><a:r><a:t>분기</a:t></a:r></a:p></c:rich></c:tx></c:title></c:catAx><c:valAx><c:title><c:tx><c:rich><a:p><a:r><a:t>금액</a:t></a:r></a:p></c:rich></c:tx></c:title><c:numFmt formatCode="General" sourceLinked="1"/></c:valAx></c:plotArea><c:legend><c:legendPos val="b"/></c:legend></c:chart></c:chartSpace>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> Option<Chart> {
        read(&super::super::xml::parse(xml.as_bytes()).unwrap(), &Colors::default())
    }

    /// The chart is what its part stores: series with their cached values (a missing point stays
    /// missing), the plot of each, titles, the data's number format, labels and legend.
    #[test]
    fn reads_series_values_and_look_from_the_part() {
        let chart = parse(SAMPLE_CHART).expect("chart");
        assert_eq!(chart.title.as_deref(), Some("분기별 실적"));
        assert_eq!((chart.plot, chart.stacking), (ChartPlot::Column, Stacking::None));
        assert_eq!(chart.categories, ["1분기", "2분기", "3분기"]);
        let series: Vec<_> = chart.series.iter().map(|s| (s.name.as_str(), s.plot, s.color.as_str())).collect();
        assert_eq!(
            series,
            [
                ("매출", ChartPlot::Column, "#4472C4"),
                ("비용", ChartPlot::Column, "#C00000"),
                // No theme here: the accent the line names is unknown, the third default colour stands in.
                ("목표", ChartPlot::Line, "#A5A5A5"),
            ]
        );
        assert_eq!(chart.series[0].values, [Some(1200.0), None, Some(1380.0)]);
        assert_eq!(chart.series[2].values, [Some(1000.0), Some(1100.0), Some(1250.0)]);
        assert_eq!((chart.category_title.as_deref(), chart.value_title.as_deref()), (Some("분기"), Some("금액")));
        assert_eq!(chart.number_format.as_deref(), Some("#,##0"), "linked to the data's format");
        assert!(chart.show_values && !chart.show_percent);
        assert_eq!((chart.legend, chart.gap_width), (Some('b'), 100.0));

        // Drawn with real text: title, axis titles, categories, round ticks, value labels, legend.
        let svg = svg(&chart, 150.0, 100.0);
        for label in
            ["분기별 실적", "분기", "금액", "1분기", "3분기", "1,000", "2,000", "1,200", "910", "매출", "비용", "목표"]
        {
            assert!(svg.contains(&format!(">{label}</text>")), "{label}: {svg}");
        }
        // Five bars (one point is missing) and three legend marks; one line through three points.
        assert_eq!(svg.matches("<rect ").count(), 5 + 3, "{svg}");
        assert_eq!(svg.matches("<circle ").count(), 3);
        assert!(svg.contains("fill=\"none\" stroke=\"#A5A5A5\""));

        // The same data as a table: categories down, series across, in the chart's number format.
        let table = data_table(&chart);
        let rows: Vec<Vec<String>> =
            table.rows.iter().map(|r| r.iter().map(|c| c.text().trim().to_string()).collect()).collect();
        assert_eq!(rows[0], ["분기", "매출", "비용", "목표"]);
        assert_eq!(rows[2], ["2분기", "", "910", "1,100"]);
    }

    #[test]
    fn pies_stacks_and_the_kinds_that_are_not_drawn() {
        let series = |name: &str, values: &[f64], extra: &str| {
            let cats: String =
                (0..values.len()).map(|i| format!(r#"<c:pt idx="{i}"><c:v>항목{}</c:v></c:pt>"#, i + 1)).collect();
            let vals: String =
                values.iter().enumerate().map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#)).collect();
            format!(
                r#"<c:ser><c:tx><c:v>{name}</c:v></c:tx>{extra}<c:cat><c:strLit>{cats}</c:strLit></c:cat><c:val><c:numLit>{vals}</c:numLit></c:val></c:ser>"#
            )
        };
        let space = |plot: &str, after: &str| {
            format!(
                r#"<c:chartSpace xmlns:c="c" xmlns:a="a"><c:chart><c:autoTitleDeleted val="1"/><c:plotArea>{plot}{after}</c:plotArea></c:chart><c:txPr><a:p><a:pPr><a:defRPr sz="1400"/></a:pPr></a:p></c:txPr></c:chartSpace>"#
            )
        };
        // A pie: one colour per point (the second set by hand), shares as labels, slices from the top.
        let point =
            r#"<c:dPt><c:idx val="1"/><c:spPr><a:solidFill><a:srgbClr val="00B050"/></a:solidFill></c:spPr></c:dPt>"#;
        let pie = format!(
            r#"<c:pieChart>{}<c:dLbls><c:showVal val="0"/><c:showPercent val="1"/></c:dLbls><c:firstSliceAng val="0"/></c:pieChart>"#,
            series("비중", &[50.0, 25.0, 25.0], point)
        );
        let chart = parse(&space(&pie, "")).expect("pie");
        assert_eq!((chart.plot, chart.title.as_deref(), chart.legend), (ChartPlot::Pie, None, None));
        assert_eq!(chart.point_colors, ["#4472C4", "#00B050", "#A5A5A5"]);
        assert_eq!(chart.font_pt, Some(14.0));
        let drawn = svg(&chart, 100.0, 100.0);
        assert_eq!(drawn.matches("<path ").count(), 3);
        assert!(drawn.contains(">50%</text>") && drawn.matches(">25%</text>").count() == 2, "{drawn}");
        // The half slice runs from the top down the right side to the bottom.
        let (w, r) = (100.0f32 * 96.0 / 25.4, 100.0f32 * 96.0 / 25.4 / 2.0 - 6.0 - 14.0 * 96.0 / 72.0 * 0.4);
        assert!(drawn.contains(&format!("<path d=\"M{:.1} {:.1}A", w / 2.0, w / 2.0 - r)), "{drawn}");

        // Horizontal bars stacked to 100 %, the axis in reverse order.
        let bars = format!(
            r#"<c:barChart><c:barDir val="bar"/><c:grouping val="percentStacked"/>{}{}</c:barChart>"#,
            series("찬성", &[30.0, 60.0], ""),
            series("반대", &[70.0, 40.0], "")
        );
        let axes = r#"<c:catAx><c:scaling><c:orientation val="maxMin"/></c:scaling></c:catAx><c:valAx><c:numFmt formatCode="0%" sourceLinked="0"/></c:valAx>"#;
        let chart = parse(&space(&bars, axes)).expect("bars");
        assert_eq!((chart.plot, chart.stacking, chart.reversed), (ChartPlot::Bar, Stacking::Percent, true));
        assert_eq!(chart.number_format.as_deref(), Some("0%"));
        let drawn = svg(&chart, 120.0, 80.0);
        assert!(drawn.contains(">100%</text>") && drawn.contains(">항목1</text>"), "{drawn}");
        assert_eq!(drawn.matches("<rect ").count(), 4);

        // A scatter chart is not drawn: its plot says so and its data makes a table.
        let scatter = r#"<c:scatterChart><c:ser><c:tx><c:v>측정</c:v></c:tx><c:xVal><c:numLit><c:pt idx="0"><c:v>1</c:v></c:pt><c:pt idx="1"><c:v>2.5</c:v></c:pt></c:numLit></c:xVal><c:yVal><c:numLit><c:pt idx="0"><c:v>10</c:v></c:pt><c:pt idx="1"><c:v>20</c:v></c:pt></c:numLit></c:yVal></c:ser></c:scatterChart>"#;
        let chart = parse(&space(scatter, "")).expect("scatter");
        assert_eq!(
            (chart.plot, chart.categories.clone()),
            (ChartPlot::Other, vec!["1".to_string(), "2.5".to_string()])
        );
        assert_eq!(data_table(&chart).rows.len(), 3);
        // A bar next to a pie cannot share axes; a part without series is no chart.
        assert_eq!(parse(&space(&format!("{bars}{pie}"), "")).map(|c| c.plot), Some(ChartPlot::Other));
        assert!(parse(&space("<c:barChart/>", "")).is_none());
    }

    #[test]
    fn numbers_follow_the_format_charts_use() {
        assert_eq!(format_number(1234.5, None), "1234.5");
        assert_eq!(format_number(1200.0, Some("General")), "1200");
        assert_eq!(format_number(0.1 + 0.2, None), "0.3");
        assert_eq!(format_number(1234567.891, Some("#,##0")), "1,234,568");
        assert_eq!(format_number(-1234.5, Some("#,##0.0")), "-1,234.5");
        assert_eq!(format_number(0.256, Some("0%")), "26%");
        assert_eq!(format_number(0.256, Some("0.0%")), "25.6%");
        assert_eq!(format_number(1500.0, Some("\"₩\"#,##0")), "₩1,500");
        assert_eq!(format_number(-0.0001, Some("0.0")), "0.0");
        assert_eq!(date_label(45_292.0, Some("yyyy-mm-dd")).as_deref(), Some("2024-01-01"));
        assert_eq!(date_label(45_292.0, Some("mmm-yy")).as_deref(), Some("2024-01"));
        assert_eq!(date_label(45_292.0, Some("#,##0")), None);
    }

    #[test]
    fn a_value_axis_is_round_and_holds_zero() {
        assert_eq!(axis(0.0, 4.5), (0.0, 5.0, 1.0));
        assert_eq!(axis(0.0, 1450.0), (0.0, 1500.0, 500.0));
        assert_eq!(axis(-30.0, 80.0), (-50.0, 100.0, 25.0));
        assert_eq!(axis(0.0, 0.0), (0.0, 1.0, 0.2));
        let (lo, hi, step) = axis(12.0, 18.0);
        assert_eq!((lo, hi, step), (0.0, 20.0, 5.0));
    }
}
