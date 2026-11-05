//! The longest common subsequence of two sequences of equal-or-not items, in linear space (Myers' middle snake, divide and conquer),
//! with a budget so that no input takes unbounded time.

/// How much work one comparison may do (steps of the search, counted over all the parts): beyond it the parts still to be compared
/// are reported as completely different, which is a correct (if not the shortest) answer.
pub const DEFAULT_BUDGET: usize = 20_000_000;

/// The pairs `(i, j)` with `a[i] == b[j]` of a longest common subsequence (strictly increasing in both), as found within the budget.
pub fn matches<T: PartialEq>(a: &[T], b: &[T], budget: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut left = budget;
    recurse(a, b, 0, 0, &mut out, &mut left, 0);
    out
}

/// The most levels of divide and conquer (each level at least halves the search; this is far beyond what the budget allows).
const MAX_DEPTH: usize = 200;

fn recurse<T: PartialEq>(a: &[T], b: &[T], ao: usize, bo: usize, out: &mut Vec<(usize, usize)>, budget: &mut usize, depth: usize) {
    // The common beginning and end are matches whatever else is found.
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    for i in 0..prefix {
        out.push((ao + i, bo + i));
    }
    let (a, b) = (a.get(prefix..).unwrap_or_default(), b.get(prefix..).unwrap_or_default());
    let (ao, bo) = (ao + prefix, bo + prefix);
    let suffix = a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count();
    let (a_mid, b_mid) = (a.get(..a.len() - suffix).unwrap_or_default(), b.get(..b.len() - suffix).unwrap_or_default());
    if !a_mid.is_empty()
        && !b_mid.is_empty()
        && depth < MAX_DEPTH
        && let Some((x, y, u, v)) = middle_snake(a_mid, b_mid, budget)
    {
        recurse(a_mid.get(..x).unwrap_or_default(), b_mid.get(..y).unwrap_or_default(), ao, bo, out, budget, depth + 1);
        for i in 0..u.saturating_sub(x) {
            out.push((ao + x + i, bo + y + i));
        }
        recurse(a_mid.get(u..).unwrap_or_default(), b_mid.get(v..).unwrap_or_default(), ao + u, bo + v, out, budget, depth + 1);
    }
    for i in 0..suffix {
        out.push((ao + a_mid.len() + i, bo + b_mid.len() + i));
    }
}

fn at<T>(s: &[T], i: i64) -> Option<&T> {
    usize::try_from(i).ok().and_then(|i| s.get(i))
}

/// A search array indexed by a diagonal that may be negative; out of range reads as 0 and writes are dropped (so no input can make it
/// panic; the sizes are chosen so that the search never leaves it).
struct Diagonals {
    cells: Vec<i64>,
    offset: i64,
}

impl Diagonals {
    fn new(max: usize) -> Self {
        Self { cells: vec![0; 2 * max + 3], offset: i64::try_from(max).unwrap_or(0) + 1 }
    }

    fn get(&self, k: i64) -> i64 {
        usize::try_from(k + self.offset).ok().and_then(|i| self.cells.get(i)).copied().unwrap_or(0)
    }

    fn set(&mut self, k: i64, value: i64) {
        if let Some(cell) = usize::try_from(k + self.offset).ok().and_then(|i| self.cells.get_mut(i)) {
            *cell = value;
        }
    }
}

/// A snake `(x, y)` to `(u, v)` (equal items from `a[x..u]` and `b[y..v]`, `u - x == v - y`) on a shortest path, the two halves of
/// which are then compared separately. None when the budget ran out.
fn middle_snake<T: PartialEq>(a: &[T], b: &[T], budget: &mut usize) -> Option<(usize, usize, usize, usize)> {
    let (n, m) = (i64::try_from(a.len()).ok()?, i64::try_from(b.len()).ok()?);
    let max = (n + m + 1) / 2;
    let delta = n - m;
    let odd = delta % 2 != 0;
    let size = usize::try_from(max).ok()?;
    let mut forward = Diagonals::new(size);
    let mut backward = Diagonals::new(size);
    for d in 0..=max {
        let mut k = -d;
        while k <= d {
            *budget = budget.checked_sub(1)?;
            let mut x = if k == -d || (k != d && forward.get(k - 1) < forward.get(k + 1)) { forward.get(k + 1) } else { forward.get(k - 1) + 1 };
            let mut y = x - k;
            let (x0, y0) = (x, y);
            while x < n && y < m && at(a, x).zip(at(b, y)).is_some_and(|(p, q)| p == q) {
                x += 1;
                y += 1;
                *budget = budget.saturating_sub(1);
            }
            forward.set(k, x);
            // The backward path on the diagonal that meets this one.
            let c = delta - k;
            if odd && (-(d - 1)..d).contains(&c) && x + backward.get(c) >= n {
                return Some((usize::try_from(x0).ok()?, usize::try_from(y0).ok()?, usize::try_from(x).ok()?, usize::try_from(y).ok()?));
            }
            k += 2;
        }
        let mut k = -d;
        while k <= d {
            *budget = budget.checked_sub(1)?;
            let mut x = if k == -d || (k != d && backward.get(k - 1) < backward.get(k + 1)) { backward.get(k + 1) } else { backward.get(k - 1) + 1 };
            let mut y = x - k;
            let (x0, y0) = (x, y);
            while x < n && y < m && at(a, n - 1 - x).zip(at(b, m - 1 - y)).is_some_and(|(p, q)| p == q) {
                x += 1;
                y += 1;
                *budget = budget.saturating_sub(1);
            }
            backward.set(k, x);
            let c = delta - k;
            if !odd && (c >= -d && c <= d) && x + forward.get(c) >= n {
                return Some((
                    usize::try_from(n - x).ok()?,
                    usize::try_from(m - y).ok()?,
                    usize::try_from(n - x0).ok()?,
                    usize::try_from(m - y0).ok()?,
                ));
            }
            k += 2;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dp_length(a: &[u8], b: &[u8]) -> usize {
        let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in 0..a.len() {
            for j in 0..b.len() {
                table[i + 1][j + 1] = if a[i] == b[j] { table[i][j] + 1 } else { table[i][j + 1].max(table[i + 1][j]) };
            }
        }
        table[a.len()][b.len()]
    }

    fn check(a: &[u8], b: &[u8]) {
        let found = matches(a, b, DEFAULT_BUDGET);
        assert!(found.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1), "increasing: {found:?}");
        assert!(found.iter().all(|&(i, j)| a[i] == b[j]), "equal items");
        assert_eq!(found.len(), dp_length(a, b), "a longest one: {a:?} {b:?}");
    }

    #[test]
    fn examples_agree_with_the_table_method() {
        check(b"", b"");
        check(b"abc", b"");
        check(b"", b"abc");
        check(b"abcabba", b"cbabac");
        check(b"abcdef", b"abcdef");
        check(b"aaaa", b"aa");
        check(b"xaybzc", b"abc");
        check(b"ABCABBA", b"CBABAC");
    }

    #[test]
    fn an_exhausted_budget_still_gives_matches_that_are_correct() {
        let a: Vec<u8> = (0..300).map(|i| (i * 7 % 13) as u8).collect();
        let b: Vec<u8> = (0..300).map(|i| (i * 5 % 11) as u8).collect();
        let found = matches(&a, &b, 50);
        assert!(found.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
        assert!(found.iter().all(|&(i, j)| a[i] == b[j]));
        assert!(found.len() <= dp_length(&a, &b));
    }

    proptest::proptest! {
        #[test]
        fn matches_are_a_longest_common_subsequence(a in proptest::collection::vec(0u8..4, 0..40), b in proptest::collection::vec(0u8..4, 0..40)) {
            check(&a, &b);
        }
    }
}
