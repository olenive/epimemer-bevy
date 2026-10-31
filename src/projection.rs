//! The topic map's projection (DESIGN.md §7, §16): the topic embeddings
//! fitted by principal component analysis and projected onto the map.
//!
//! [`fit`] centres the vectors on their mean, forms their covariance, and
//! finds its two leading eigenvectors by power iteration, the second on the
//! covariance with the first deflated out of it. Each component's sign is
//! set so that its largest entry is positive, since an eigenvector is only
//! known up to its sign, and a refit over the same vectors must give the
//! same map rather than its mirror image. The arithmetic is in `f64` and
//! starts from a fixed vector, so the same vectors always give the same
//! projection, bit for bit.
//!
//! The projected points are then scaled by one factor, so the map keeps
//! the aspect the two components give it: [`EXTENT_SHARE`] of the fitted
//! points lie within the radius asked for, and the rest, the outliers a
//! projection of sentence embeddings always has, beyond it.
//!
//! The fitted [`Projection`] is a value: the model it was fitted under,
//! the mean, the two components and the scale, all serialisable, so it is
//! saved with the map (`persist.rs`). [`project`] puts any vector of the
//! same model through it, which is how a topic that arrives after the fit
//! lands where its content puts it without anything being fitted again.

use bevy::math::Vec2;
use serde::{Deserialize, Serialize};

/// The most power iterations for one component. The development graph's
/// third eigenvalue is 0.85 of its second, so the second component needs
/// about 150 to settle; this is room for a far closer pair.
pub const MAX_ITERATIONS: usize = 5_000;

/// A component has settled when one iteration moves it by less than this,
/// as the squared distance between the unit vectors before and after.
pub const SETTLED: f64 = 1e-20;

/// The share of the fitted points the scale puts within the radius asked
/// for.
pub const EXTENT_SHARE: f64 = 0.95;

/// A fitted projection from a model's embedding space onto the map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    /// The embedding model the vectors came from; a vector of another model
    /// cannot be projected.
    pub model_id: String,
    /// The mean of the fitted vectors.
    pub mean: Vec<f32>,
    /// The two leading principal components, unit vectors: the map's x and
    /// its other axis.
    pub components: [Vec<f32>; 2],
    /// Map units per unit along a component.
    pub scale: f32,
}

impl Projection {
    /// The length of the vectors it takes.
    pub fn dimension(&self) -> usize {
        self.mean.len()
    }
}

/// Fits a projection to `vectors`, all of model `model_id`, scaled so that
/// [`EXTENT_SHARE`] of them land within `radius` of the map's middle. Every
/// vector must have the same length, at least two, and finite entries.
pub fn fit(model_id: &str, vectors: &[&[f32]], radius: f32) -> Result<Projection, String> {
    let first = vectors.first().ok_or("there are no vectors to fit")?;
    let d = first.len();
    if d < 2 {
        return Err(format!(
            "vectors of {d} dimension cannot make a plane; two at least are needed"
        ));
    }
    if let Some(odd) = vectors.iter().find(|v| v.len() != d) {
        return Err(format!(
            "the vectors differ in length: {d} and {}",
            odd.len()
        ));
    }
    if vectors.iter().any(|v| v.iter().any(|x| !x.is_finite())) {
        return Err("a vector holds a value that is not a finite number".into());
    }
    let n = vectors.len() as f64;
    let mut mean = vec![0.0_f64; d];
    for v in vectors {
        for (m, &x) in mean.iter_mut().zip(*v) {
            *m += x as f64;
        }
    }
    for m in &mut mean {
        *m /= n;
    }
    let mut covariance = covariance(vectors, &mean);
    let first = leading(&covariance, d, None);
    deflate(&mut covariance, d, &first.0, first.1);
    let second = leading(&covariance, d, Some(&first.0));

    let narrow = |v: &[f64]| v.iter().map(|&x| x as f32).collect::<Vec<f32>>();
    let unscaled = Projection {
        model_id: model_id.to_owned(),
        mean: narrow(&mean),
        components: [narrow(&first.0), narrow(&second.0)],
        scale: 1.0,
    };
    let mut radii: Vec<f64> = vectors
        .iter()
        .filter_map(|v| along(&unscaled, v))
        .map(|(x, y)| x.hypot(y))
        .collect();
    radii.sort_by(f64::total_cmp);
    let at = ((EXTENT_SHARE * radii.len() as f64).ceil() as usize).clamp(1, radii.len()) - 1;
    let extent = radii[at];
    let scale = if extent > 0.0 && radius > 0.0 {
        (radius as f64 / extent) as f32
    } else {
        1.0
    };
    Ok(Projection { scale, ..unscaled })
}

/// Where `vector` lands on the map, or `None` when its length is not the
/// projection's.
pub fn project(projection: &Projection, vector: &[f32]) -> Option<Vec2> {
    let (x, y) = along(projection, vector)?;
    let scale = projection.scale as f64;
    Some(Vec2::new((x * scale) as f32, (y * scale) as f32))
}

/// A vector's coordinates along the two components, before the scale.
fn along(projection: &Projection, vector: &[f32]) -> Option<(f64, f64)> {
    if vector.len() != projection.dimension() {
        return None;
    }
    let dot = |component: &[f32]| {
        vector
            .iter()
            .zip(&projection.mean)
            .zip(component)
            .map(|((&v, &m), &c)| (v as f64 - m as f64) * c as f64)
            .sum::<f64>()
    };
    Some((
        dot(&projection.components[0]),
        dot(&projection.components[1]),
    ))
}

/// The covariance of the vectors about `mean`, `d` by `d`, row by row.
fn covariance(vectors: &[&[f32]], mean: &[f64]) -> Vec<f64> {
    let d = mean.len();
    let mut sums = vec![0.0_f64; d * d];
    let mut centred = vec![0.0_f64; d];
    for v in vectors {
        for ((c, &x), &m) in centred.iter_mut().zip(*v).zip(mean) {
            *c = x as f64 - m;
        }
        // The upper triangle only; the lower is its mirror.
        for i in 0..d {
            let ci = centred[i];
            if ci == 0.0 {
                continue;
            }
            let row = &mut sums[i * d + i..(i + 1) * d];
            for (s, &cj) in row.iter_mut().zip(&centred[i..]) {
                *s += ci * cj;
            }
        }
    }
    let n = vectors.len() as f64;
    for i in 0..d {
        for j in i..d {
            let value = sums[i * d + j] / n;
            sums[i * d + j] = value;
            sums[j * d + i] = value;
        }
    }
    sums
}

/// The covariance with a component and its eigenvalue taken out of it.
fn deflate(matrix: &mut [f64], d: usize, component: &[f64], value: f64) {
    for i in 0..d {
        for j in 0..d {
            matrix[i * d + j] -= value * component[i] * component[j];
        }
    }
}

/// The leading eigenvector of a symmetric `d` by `d` matrix and its
/// eigenvalue, by power iteration from a fixed start, kept orthogonal to
/// `against` where that is given. A matrix with no variance left gives a
/// unit vector all the same: the first basis vector orthogonal to
/// `against`, with eigenvalue 0.
fn leading(matrix: &[f64], d: usize, against: Option<&[f64]>) -> (Vec<f64>, f64) {
    let mut v = start(d);
    if let Some(a) = against {
        remove(&mut v, a);
    }
    if !normalise(&mut v) {
        return (basis(d, against), 0.0);
    }
    for _ in 0..MAX_ITERATIONS {
        let mut next = times(matrix, d, &v);
        if let Some(a) = against {
            remove(&mut next, a);
        }
        if !normalise(&mut next) {
            return (basis(d, against), 0.0);
        }
        let change: f64 = next.iter().zip(&v).map(|(a, b)| (a - b) * (a - b)).sum();
        v = next;
        if change < SETTLED {
            break;
        }
    }
    let value = v.iter().zip(times(matrix, d, &v)).map(|(a, b)| a * b).sum();
    set_sign(&mut v);
    (v, value)
}

/// A fixed start for power iteration: entries between -1 and 1 from a
/// xorshift generator with a fixed seed, so that no eigenvector is
/// orthogonal to it except by a coincidence the data would have to aim for.
fn start(d: usize) -> Vec<f64> {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..d)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        })
        .collect()
}

fn times(matrix: &[f64], d: usize, v: &[f64]) -> Vec<f64> {
    matrix
        .chunks_exact(d)
        .map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum())
        .collect()
}

/// Takes the part along the unit vector `a` out of `v`.
fn remove(v: &mut [f64], a: &[f64]) {
    let along: f64 = v.iter().zip(a).map(|(x, y)| x * y).sum();
    for (x, y) in v.iter_mut().zip(a) {
        *x -= along * y;
    }
}

/// Scales `v` to unit length; false when it has none to scale.
fn normalise(v: &mut [f64]) -> bool {
    let length = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if !length.is_finite() || length <= 1e-150 {
        return false;
    }
    for x in v.iter_mut() {
        *x /= length;
    }
    true
}

/// The first basis vector with a part orthogonal to `against`, that part
/// as a unit vector.
fn basis(d: usize, against: Option<&[f64]>) -> Vec<f64> {
    (0..d)
        .find_map(|k| {
            let mut v = vec![0.0; d];
            v[k] = 1.0;
            if let Some(a) = against {
                remove(&mut v, a);
            }
            normalise(&mut v).then(|| {
                set_sign(&mut v);
                v
            })
        })
        .unwrap_or_else(|| vec![0.0; d])
}

/// Flips `v` so that its entry of largest magnitude, the first of them on
/// a tie, is positive.
fn set_sign(v: &mut [f64]) {
    let largest = v.iter().copied().fold(
        0.0_f64,
        |best, x| if x.abs() > best.abs() { x } else { best },
    );
    if largest < 0.0 {
        for x in v.iter_mut() {
            *x = -*x;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fixed stream of numbers between -1 and 1.
    pub(crate) fn noise(seed: u64) -> impl FnMut() -> f32 {
        let mut state = seed.max(1);
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 11) as f64 / (1u64 << 52) as f64 - 1.0) as f32
        }
    }

    fn unit(v: [f32; 5]) -> [f32; 5] {
        let length = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.map(|x| x / length)
    }

    /// A cloud in five dimensions about `centre`, long along `a`, half as
    /// wide along `b` and thin across the rest.
    fn elongated() -> (Vec<Vec<f32>>, [f32; 5], [f32; 5], [f32; 5]) {
        let a = unit([1.0, 1.0, 0.0, 0.0, 0.0]);
        let b = unit([0.0, 0.0, 1.0, -1.0, 0.0]);
        let centre = [3.0, -2.0, 0.5, 1.0, 7.0];
        let mut r = noise(42);
        let points = (0..400)
            .map(|_| {
                let (s, t) = (10.0 * r(), 5.0 * r());
                let mut p = centre;
                for k in 0..5 {
                    p[k] += s * a[k] + t * b[k] + 0.2 * r();
                }
                p.to_vec()
            })
            .collect();
        (points, a, b, centre)
    }

    fn dot(x: &[f32], y: &[f32]) -> f32 {
        x.iter().zip(y).map(|(a, b)| a * b).sum()
    }

    fn slices(points: &[Vec<f32>]) -> Vec<&[f32]> {
        points.iter().map(Vec::as_slice).collect()
    }

    #[test]
    fn the_fit_recovers_the_axes_of_an_elongated_cloud() {
        let (points, a, b, centre) = elongated();
        let projection = fit("model", &slices(&points), 100.0).unwrap();
        let [first, second] = &projection.components;
        assert!(dot(first, &a).abs() > 0.999, "{first:?}");
        assert!(dot(second, &b).abs() > 0.999, "{second:?}");
        assert!(dot(first, second).abs() < 1e-5, "orthogonal");
        for component in [first, second] {
            assert!((dot(component, component) - 1.0).abs() < 1e-5, "unit");
            let largest = component.iter().copied().fold(0.0_f32, |best, x| {
                if x.abs() > best.abs() { x } else { best }
            });
            assert!(largest > 0.0, "its largest entry is positive");
        }
        for (m, c) in projection.mean.iter().zip(centre) {
            assert!((m - c).abs() < 0.5, "{m} against {c}");
        }
        assert_eq!(projection.model_id, "model");
    }

    #[test]
    fn the_fit_is_the_same_every_time() {
        let (points, ..) = elongated();
        let once = fit("model", &slices(&points), 100.0).unwrap();
        let again = fit("model", &slices(&points), 100.0).unwrap();
        assert_eq!(once, again);
        let text = serde_json::to_string(&once).unwrap();
        let back: Projection = serde_json::from_str(&text).unwrap();
        assert_eq!(back, once, "a saved projection reads back as it was");
    }

    #[test]
    fn the_scale_puts_the_bulk_within_the_radius_and_keeps_the_aspect() {
        let (points, ..) = elongated();
        let projection = fit("model", &slices(&points), 100.0).unwrap();
        let radii: Vec<f32> = points
            .iter()
            .map(|p| project(&projection, p).unwrap().length())
            .collect();
        let within = radii.iter().filter(|&&r| r <= 100.0 + 1e-3).count();
        assert_eq!(within, (0.95_f64 * 400.0).ceil() as usize);
        // A step along the long axis and the same step along the short
        // one land the same distance from the middle, each on its own axis
        // of the map.
        let at = |component: usize, step: f32| {
            let axis = &projection.components[component];
            let p: Vec<f32> = projection
                .mean
                .iter()
                .zip(axis)
                .map(|(m, c)| m + step * c)
                .collect();
            project(&projection, &p).unwrap()
        };
        let (long, short) = (at(0, 4.0), at(1, 4.0));
        assert!((long.length() - short.length()).abs() < 1e-4 * long.length());
        assert!((long.x - 4.0 * projection.scale).abs() < 1e-3, "{long}");
        assert!(long.y.abs() < 1e-3, "{long}");
        assert!((short.y - 4.0 * projection.scale).abs() < 1e-3, "{short}");
        assert!(short.x.abs() < 1e-3, "{short}");
    }

    #[test]
    fn a_new_point_lands_where_its_content_puts_it() {
        let (points, a, _, _) = elongated();
        let projection = fit("model", &slices(&points), 100.0).unwrap();
        // A fitted vector projected again lands where the fit put it.
        let again = project(&projection, &points[17]).unwrap();
        assert_eq!(again, project(&projection, &points[17]).unwrap());
        // A new vector at the mean, five units along the first component
        // and two along the second, lands at five and two times the scale;
        // moved along the thin fifth axis, which the map does not show, it
        // lands in nearly the same place.
        let [first, second] = &projection.components;
        let new: Vec<f32> = (0..5)
            .map(|k| projection.mean[k] + 5.0 * first[k] + 2.0 * second[k])
            .collect();
        let mut off = new.clone();
        off[4] += 3.0;
        let expected = Vec2::new(5.0, 2.0) * projection.scale;
        let landed = project(&projection, &new).unwrap();
        assert!(
            landed.distance(expected) < 1e-3,
            "{landed} against {expected}"
        );
        let beside = project(&projection, &off).unwrap();
        assert!(
            beside.distance(expected) < 0.05 * projection.scale,
            "{beside}"
        );
        // Content along the cloud's long axis lands along the map's x.
        let long: Vec<f32> = (0..5).map(|k| projection.mean[k] + 6.0 * a[k]).collect();
        let along = project(&projection, &long).unwrap();
        assert!(along.y.abs() < 0.1 * along.x.abs(), "{along}");
        assert_eq!(project(&projection, &[1.0, 2.0]), None, "another length");
    }

    #[test]
    fn too_few_or_mismatched_vectors_are_refused_and_a_single_one_is_the_middle() {
        assert!(fit("m", &[], 10.0).is_err());
        assert!(fit("m", &[&[1.0]], 10.0).unwrap_err().contains("plane"));
        assert!(fit("m", &[&[1.0, 2.0], &[1.0, 2.0, 3.0]], 10.0).is_err());
        assert!(fit("m", &[&[1.0, f32::NAN]], 10.0).is_err());
        let one = fit("m", &[&[1.0, 2.0, 3.0]], 10.0).unwrap();
        assert_eq!(project(&one, &[1.0, 2.0, 3.0]), Some(Vec2::ZERO));
        assert!((dot(&one.components[0], &one.components[1])).abs() < 1e-6);
        let two = fit("m", &[&[0.0, 0.0, 0.0], &[2.0, 0.0, 0.0]], 10.0).unwrap();
        let ends = [
            project(&two, &[0.0, 0.0, 0.0]).unwrap(),
            project(&two, &[2.0, 0.0, 0.0]).unwrap(),
        ];
        assert!(
            (ends[0].x + 10.0).abs() < 1e-4 && (ends[1].x - 10.0).abs() < 1e-4,
            "{ends:?}"
        );
    }
}
