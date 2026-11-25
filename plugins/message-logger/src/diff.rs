//! The text diff of Equicord's MessageLogger (diffUtils.ts `createWordDiff`): the simple cases (text added or removed at
//! one end) directly, everything else as a longest common subsequence over characters, grouped into runs.

#[derive(Clone, Debug, PartialEq)]
pub enum Part { Same(String), Added(String), Removed(String) }
impl Part {
    pub fn text(&self) -> &str { match self { Part::Same(t) | Part::Added(t) | Part::Removed(t) => t } }
    /// The span kind Webox reads ("" for unchanged text).
    pub fn kind(&self) -> &'static str { match self { Part::Same(_) => "", Part::Added(_) => "added", Part::Removed(_) => "removed" } }
    fn same_kind(&self, o: &Part) -> bool { std::mem::discriminant(self) == std::mem::discriminant(o) }
    fn push(&mut self, c: &str) { match self { Part::Same(t) | Part::Added(t) | Part::Removed(t) => t.push_str(c) } }
}

/// Above this many table cells (old length times new length) the texts count as replaced as a whole, so a huge edit
/// cannot stall the window (the events run on its thread).
const LCS_MAX: usize = 4_000_000;

/// How `old` became `new`.
pub fn diff(old: &str, new: &str) -> Vec<Part> {
    let both = |a: Part, b: Part| [a, b].into_iter().filter(|p| !p.text().is_empty()).collect::<Vec<_>>();
    if old == new { return both(Part::Same(old.into()), Part::Same(String::new())); }
    if old.len() < new.len() {
        if let Some(rest) = new.strip_prefix(old) { return both(Part::Same(old.into()), Part::Added(rest.into())); }
        if let Some(rest) = new.strip_suffix(old) { return both(Part::Added(rest.into()), Part::Same(old.into())); }
    }
    if new.len() < old.len() {
        if let Some(rest) = old.strip_prefix(new) { return both(Part::Same(new.into()), Part::Removed(rest.into())); }
        if let Some(rest) = old.strip_suffix(new) { return both(Part::Removed(rest.into()), Part::Same(new.into())); }
    }
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    if (a.len() + 1) * (b.len() + 1) > LCS_MAX { return both(Part::Removed(old.into()), Part::Added(new.into())); }
    // dp[i][j]: the longest common subsequence of a[..i] and b[..j].
    let w = b.len() + 1;
    let mut dp = vec![0u32; (a.len() + 1) * w];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            dp[i * w + j] = if a[i - 1] == b[j - 1] { dp[(i - 1) * w + j - 1] + 1 } else { dp[(i - 1) * w + j].max(dp[i * w + j - 1]) };
        }
    }
    let (mut i, mut j) = (a.len(), b.len());
    let mut rev: Vec<Part> = Vec::new();
    while i > 0 || j > 0 {
        // On a tie the added character is taken first going backwards, so a replacement reads "removed, then added".
        let p = if i > 0 && j > 0 && a[i - 1] == b[j - 1] { i -= 1; j -= 1; Part::Same(a[i].to_string()) }
            else if j > 0 && (i == 0 || dp[i * w + j - 1] >= dp[(i - 1) * w + j]) { j -= 1; Part::Added(b[j].to_string()) }
            else { i -= 1; Part::Removed(a[i].to_string()) };
        rev.push(p);
    }
    let mut out: Vec<Part> = Vec::new();
    for p in rev.into_iter().rev() {
        match out.last_mut() { Some(l) if l.same_kind(&p) => l.push(p.text()), _ => out.push(p) }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn show(p: &[Part]) -> String { p.iter().map(|p| match p { Part::Same(t) => t.clone(), Part::Added(t) => format!("[+{t}]"), Part::Removed(t) => format!("[-{t}]") }).collect() }

    #[test]
    fn simple_ends_and_the_general_case() {
        assert_eq!(show(&diff("watch", "watch this")), "watch[+ this]");
        assert_eq!(show(&diff("this", "watch this")), "[+watch ]this");
        assert_eq!(show(&diff("watch this", "watch")), "watch[- this]");
        assert_eq!(show(&diff("prefix watch", "watch")), "[-prefix ]watch");
        assert_eq!(show(&diff("", "neu")), "[+neu]");
        assert_eq!(show(&diff("gleich", "gleich")), "gleich");
        assert_eq!(show(&diff("Hallo Welt", "Hallo Leute")), "Hallo [-W][+L]e[-l][+u]t[+e]");
        // Emoji and umlauts are single characters, never split.
        assert_eq!(show(&diff("Grüße 👋", "Grüße 🙂")), "Grüße [-👋][+🙂]");
    }

    #[test]
    fn huge_texts_count_as_replaced() {
        let (a, b) = ("a".repeat(3000), "b".repeat(3000));
        assert_eq!(diff(&a, &b), [Part::Removed(a.clone()), Part::Added(b.clone())]);
    }
}
