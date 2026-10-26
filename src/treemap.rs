//! Squarified treemap layout, independent of egui and platform storage APIs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Rect {
    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
    pub fn area(self) -> f64 {
        self.width * self.height
    }
}

/// Zero weights have no area. Their objects remain available in the equivalent list.
/// Callers group small objects before layout to bound painting and hit testing.
pub fn squarify(weights: &[f64], bounds: Rect) -> Vec<(usize, Rect)> {
    if bounds.width <= 0.0 || bounds.height <= 0.0 || !bounds.area().is_finite() {
        return Vec::new();
    }
    let mut items: Vec<_> = weights
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, w)| w.is_finite() && *w > 0.0)
        .collect();
    let total: f64 = items.iter().map(|(_, w)| w).sum();
    if total <= 0.0 || !total.is_finite() {
        return Vec::new();
    }
    items.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    for (_, weight) in &mut items {
        *weight = *weight / total * bounds.area();
    }
    let mut remaining = bounds;
    let mut output = Vec::with_capacity(items.len());
    let mut start = 0;
    while start < items.len() {
        let side = remaining.width.min(remaining.height);
        if side <= 0.0 {
            break;
        }
        let mut end = start + 1;
        while end < items.len()
            && worst(&items[start..=end], side) <= worst(&items[start..end], side)
        {
            end += 1;
        }
        let row = &items[start..end];
        let sum: f64 = row.iter().map(|(_, a)| a).sum();
        if remaining.width >= remaining.height {
            let width = (sum / remaining.height).min(remaining.width);
            let mut y = remaining.y;
            for (position, (index, area)) in row.iter().enumerate() {
                let height = if position + 1 == row.len() {
                    remaining.y + remaining.height - y
                } else {
                    area / width
                };
                output.push((
                    *index,
                    Rect {
                        x: remaining.x,
                        y,
                        width,
                        height,
                    },
                ));
                y += height;
            }
            remaining.x += width;
            remaining.width = (remaining.width - width).max(0.0);
        } else {
            let height = (sum / remaining.width).min(remaining.height);
            let mut x = remaining.x;
            for (position, (index, area)) in row.iter().enumerate() {
                let width = if position + 1 == row.len() {
                    remaining.x + remaining.width - x
                } else {
                    area / height
                };
                output.push((
                    *index,
                    Rect {
                        x,
                        y: remaining.y,
                        width,
                        height,
                    },
                ));
                x += width;
            }
            remaining.y += height;
            remaining.height = (remaining.height - height).max(0.0);
        }
        start = end;
    }
    output
}
fn worst(row: &[(usize, f64)], side: f64) -> f64 {
    let sum: f64 = row.iter().map(|(_, a)| a).sum();
    let min = row.iter().map(|(_, a)| *a).fold(f64::INFINITY, f64::min);
    let max = row.iter().map(|(_, a)| *a).fold(0.0, f64::max);
    (side * side * max / (sum * sum)).max(sum * sum / (side * side * min))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_preserves_area_weights_and_disjoint_hit_regions() {
        for (width, height) in [(900.0, 500.0), (320.0, 720.0), (1.0, 1.0)] {
            let bounds = Rect {
                x: 3.0,
                y: 7.0,
                width,
                height,
            };
            let weights = [40.0, 25.0, 20.0, 10.0, 5.0, 0.0, f64::NAN];
            let result = squarify(&weights, bounds);
            assert_eq!(result.len(), 5);
            for (index, rect) in &result {
                assert!((rect.area() / bounds.area() - weights[*index] / 100.0).abs() < 1e-9);
                assert!(rect.x >= bounds.x && rect.y >= bounds.y);
                assert!(rect.x + rect.width <= bounds.x + width + 1e-9);
                assert!(rect.y + rect.height <= bounds.y + height + 1e-9);
                assert_eq!(
                    result
                        .iter()
                        .filter(|(_, r)| r
                            .contains(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0))
                        .count(),
                    1
                );
            }
        }
    }
    #[test]
    fn empty_unknown_and_invalid_bounds_do_not_create_fake_area() {
        assert!(
            squarify(
                &[0.0, -1.0, f64::NAN],
                Rect {
                    width: 1.0,
                    height: 1.0,
                    ..Rect::default()
                }
            )
            .is_empty()
        );
        assert!(squarify(&[1.0], Rect::default()).is_empty());
    }
}
