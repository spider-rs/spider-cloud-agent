//! Version one numeric model artifacts. All integers and floats are little endian.
use crate::features::{EDIT_DIM, EDIT_FEATURE_VERSION};
use crate::model::ModelVersion;
use crate::schema::SCHEMA_VERSION;
use spider_route::features::FEATURES_USED;
use std::fmt;

/// Maximum layer width or tree node count accepted by this reader.
pub const MAX_WIDTH: usize = 256;
/// Pack four numeric categories without overlapping their bits.
pub const fn cell_id(need: u8, ext: u8, memory_state: u8, edit_code: u8) -> u32 {
    ((need as u32) << 24) | ((ext as u32) << 16) | ((memory_state as u32) << 8) | edit_code as u32
}
/// The inference payload family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// Three dense networks.
    Mlp,
    /// Three ensembles of decision trees.
    Gbdt,
}
/// Why an artifact cannot be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactError {
    /// Missing header or checksum.
    TooShort,
    /// Incorrect signature.
    BadMagic,
    /// Unknown artifact version.
    Version(u8),
    /// Unknown payload kind.
    Kind(u8),
    /// Incompatible base feature version.
    FeatureVersion(u16),
    /// Incompatible edit feature version.
    EditFeatureVersion(u16),
    /// Incompatible schema version.
    SchemaVersion(u16),
    /// A declared table exceeds remaining bytes.
    Truncated,
    /// Invalid scratch bound.
    Width(u16),
    /// Checksum mismatch.
    Crc,
    /// Input exceeds 2,000,000 bytes.
    Size(usize),
    /// Invalid dimensions, numeric values, topology, or trailing data.
    Shape,
}
impl fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid optimizer artifact: {self:?}")
    }
}
impl std::error::Error for ArtifactError {}

pub(crate) struct Layer {
    pub rows: usize,
    pub cols: usize,
    pub activation: u8,
    pub weights: Vec<f32>,
    pub bias: Vec<f32>,
}
pub(crate) struct Node {
    pub feature: u16,
    pub threshold: f32,
    pub left: u16,
    pub right: u16,
    pub value: f32,
}
pub(crate) enum Head {
    Mlp(Vec<Layer>),
    Gbdt(f32, Vec<Vec<Node>>),
}
pub(crate) enum Calibration {
    Identity,
    Platt(f32, f32),
    Isotonic(Vec<(f32, f32)>),
}
/// Owned, validated weights. Loading allocates; scoring does not.
pub struct Compact {
    pub(crate) kind: ModelKind,
    pub(crate) thresholds: Vec<f32>,
    pub(crate) support: Vec<u32>,
    pub(crate) calibration: Vec<Calibration>,
    pub(crate) heads: Vec<Head>,
}
struct Reader<'a> {
    bytes: &'a [u8],
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], ArtifactError> {
        let out = self.bytes.get(..n).ok_or(ArtifactError::Truncated)?;
        self.bytes = self.bytes.get(n..).ok_or(ArtifactError::Truncated)?;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, ArtifactError> {
        Ok(*self.take(1)?.first().ok_or(ArtifactError::Truncated)?)
    }
    fn u16(&mut self) -> Result<u16, ArtifactError> {
        let [low, high]: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| ArtifactError::Truncated)?;
        Ok(low as u16 | ((high as u16) << 8))
    }
    fn u32(&mut self) -> Result<u32, ArtifactError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| ArtifactError::Truncated)?,
        ))
    }
    fn float(&mut self) -> Result<f32, ArtifactError> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn finite(&mut self) -> Result<f32, ArtifactError> {
        let v = self.float()?;
        if v.is_finite() {
            Ok(v)
        } else {
            Err(ArtifactError::Shape)
        }
    }
    fn count(&self, n: usize, size: usize) -> Result<(), ArtifactError> {
        if n > self.bytes.len() / size {
            Err(ArtifactError::Truncated)
        } else {
            Ok(())
        }
    }
    fn floats(&mut self, n: usize) -> Result<Vec<f32>, ArtifactError> {
        self.count(n, 4)?;
        (0..n).map(|_| self.finite()).collect()
    }
}
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
impl Compact {
    /// Validate lengths, versions, checksum, shapes and acyclic tree topology.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ArtifactError> {
        if bytes.len() > 2_000_000 {
            return Err(ArtifactError::Size(bytes.len()));
        }
        if bytes.len() < 20 {
            return Err(ArtifactError::TooShort);
        }
        let mut r = Reader { bytes };
        if r.take(5)? != b"SPOPT" {
            return Err(ArtifactError::BadMagic);
        }
        let v = r.u8()?;
        if v != 1 {
            return Err(ArtifactError::Version(v));
        }
        let kind = match r.u8()? {
            1 => ModelKind::Mlp,
            2 => ModelKind::Gbdt,
            k => return Err(ArtifactError::Kind(k)),
        };
        let v = r.u16()?;
        if v != 1 {
            return Err(ArtifactError::FeatureVersion(v));
        }
        let v = r.u16()?;
        if v != EDIT_FEATURE_VERSION {
            return Err(ArtifactError::EditFeatureVersion(v));
        }
        let v = r.u16()?;
        if v != SCHEMA_VERSION {
            return Err(ArtifactError::SchemaVersion(v));
        }
        let width = r.u16()?;
        if width == 0 || usize::from(width) > MAX_WIDTH {
            return Err(ArtifactError::Width(width));
        }
        if r.u8()? != 0 {
            return Err(ArtifactError::Shape);
        }
        let end = bytes.len() - 4;
        let data = bytes.get(..end).ok_or(ArtifactError::Truncated)?;
        let checksum = bytes.get(end..).ok_or(ArtifactError::Truncated)?;
        if crc32(data) != (Reader { bytes: checksum }).u32()? {
            return Err(ArtifactError::Crc);
        }
        r.bytes = bytes.get(16..end).ok_or(ArtifactError::Truncated)?;
        let n = usize::from(r.u16()?);
        if n == 0 || n > 10 {
            return Err(ArtifactError::Shape);
        }
        r.count(n, 4)?;
        let mut thresholds = Vec::with_capacity(n);
        for _ in 0..n {
            let v = r.float()?;
            if !v.is_nan() && !(0.0..=1.0).contains(&v) {
                return Err(ArtifactError::Shape);
            }
            thresholds.push(v);
        }
        let n = r.u32()? as usize;
        r.count(n, 4)?;
        let mut support = Vec::with_capacity(n);
        for _ in 0..n {
            let cell = r.u32()?;
            if support.last().is_some_and(|last| *last >= cell) {
                return Err(ArtifactError::Shape);
            }
            support.push(cell);
        }
        let mut calibration = Vec::with_capacity(3);
        for _ in 0..3 {
            let k = r.u16()?;
            let n = usize::from(r.u16()?);
            calibration.push(match (k, n) {
                (0, 0) => Calibration::Identity,
                (1, 0) => Calibration::Platt(r.finite()?, r.finite()?),
                (2, 2..) => {
                    r.count(n, 8)?;
                    let mut pairs: Vec<(f32, f32)> = Vec::with_capacity(n);
                    for _ in 0..n {
                        let pair = (r.finite()?, r.finite()?);
                        if pairs.last().is_some_and(|p| p.0 >= pair.0 || p.1 > pair.1) {
                            return Err(ArtifactError::Shape);
                        }
                        pairs.push(pair);
                    }
                    Calibration::Isotonic(pairs)
                }
                _ => return Err(ArtifactError::Shape),
            });
        }
        let mut heads = Vec::with_capacity(3);
        let mut largest = 0;
        for _ in 0..3 {
            heads.push(match kind {
                ModelKind::Mlp => {
                    let n = usize::from(r.u16()?);
                    r.count(n, 5)?;
                    if n == 0 {
                        return Err(ArtifactError::Shape);
                    }
                    let mut layers = Vec::with_capacity(n);
                    let mut previous = FEATURES_USED + EDIT_DIM;
                    for _ in 0..n {
                        let rows = usize::from(r.u16()?);
                        let cols = usize::from(r.u16()?);
                        let activation = r.u8()?;
                        if rows == 0
                            || rows > usize::from(width)
                            || cols != previous
                            || cols > MAX_WIDTH
                            || activation > 2
                        {
                            return Err(ArtifactError::Shape);
                        }
                        largest = largest.max(rows).max(cols);
                        layers.push(Layer {
                            rows,
                            cols,
                            activation,
                            weights: r.floats(rows * cols)?,
                            bias: r.floats(rows)?,
                        });
                        previous = rows;
                    }
                    if previous != 1 {
                        return Err(ArtifactError::Shape);
                    }
                    Head::Mlp(layers)
                }
                ModelKind::Gbdt => {
                    let n = r.u32()? as usize;
                    let base = r.finite()?;
                    r.count(n, 2)?;
                    let mut trees = Vec::with_capacity(n);
                    for _ in 0..n {
                        let n = usize::from(r.u16()?);
                        if n == 0 || n > usize::from(width) {
                            return Err(ArtifactError::Shape);
                        }
                        largest = largest.max(n);
                        r.count(n, 14)?;
                        let mut nodes = Vec::with_capacity(n);
                        for _ in 0..n {
                            nodes.push(Node {
                                feature: r.u16()?,
                                threshold: r.finite()?,
                                left: r.u16()?,
                                right: r.u16()?,
                                value: r.finite()?,
                            });
                        }
                        for node in &nodes {
                            if node.left == 65535 && node.right == 65535 {
                                continue;
                            }
                            if usize::from(node.feature & 0x7fff) >= FEATURES_USED + EDIT_DIM
                                || usize::from(node.left) >= n
                                || usize::from(node.right) >= n
                            {
                                return Err(ArtifactError::Shape);
                            }
                        }
                        // DFS colours detect cycles even in unreachable subgraphs.
                        let mut colours = vec![0u8; n];
                        for at in 0..n {
                            visit(&nodes, &mut colours, at)?;
                        }
                        trees.push(nodes);
                    }
                    Head::Gbdt(base, trees)
                }
            });
        }
        if largest != usize::from(width) || !r.bytes.is_empty() {
            return Err(ArtifactError::Shape);
        }
        Ok(Self {
            kind,
            thresholds,
            support,
            calibration,
            heads,
        })
    }
    /// Payload family.
    pub fn kind(&self) -> ModelKind {
        self.kind
    }
    /// Artifact version.
    pub fn version(&self) -> ModelVersion {
        ModelVersion(1)
    }
    /// Success floor; missing codes and NaN entries abstain.
    pub fn threshold(&self, edit_code: u8) -> Option<f32> {
        self.thresholds
            .get(usize::from(edit_code))
            .copied()
            .filter(|v| v.is_finite())
    }
    /// Whether this exact cell occurs in the support table.
    pub fn supported(&self, cell: u32) -> bool {
        self.support.binary_search(&cell).is_ok()
    }
}
fn visit(nodes: &[Node], colours: &mut [u8], at: usize) -> Result<(), ArtifactError> {
    match colours.get(at) {
        Some(2) => return Ok(()),
        Some(0) => {}
        _ => return Err(ArtifactError::Shape),
    }
    *colours.get_mut(at).ok_or(ArtifactError::Shape)? = 1;
    let node = nodes.get(at).ok_or(ArtifactError::Shape)?;
    if node.left != 65535 || node.right != 65535 {
        visit(nodes, colours, usize::from(node.left))?;
        visit(nodes, colours, usize::from(node.right))?;
    }
    *colours.get_mut(at).ok_or(ArtifactError::Shape)? = 2;
    Ok(())
}
/// Load the bundled synthetic fixture, not a trained production model.
#[cfg(feature = "embedded-model")]
pub fn embedded() -> Result<Compact, ArtifactError> {
    Compact::from_bytes(include_bytes!("../assets/spider-optimize-v1.bin"))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;
    use crate::{edit_code, Input, Key, Schema, Scorer, KEEP_CODE};

    pub(crate) struct ArtifactTables {
        model: Compact,
        width: u16,
    }
    fn u16out(out: &mut Vec<u8>, x: u16) {
        out.extend(x.to_le_bytes());
    }
    fn u32out(out: &mut Vec<u8>, x: u32) {
        out.extend(x.to_le_bytes());
    }
    fn float(out: &mut Vec<u8>, x: f32) {
        u32out(out, x.to_bits());
    }
    pub(crate) fn write_artifact(tables: &ArtifactTables) -> Vec<u8> {
        let model = &tables.model;
        let mut out = b"SPOPT".to_vec();
        out.push(1);
        out.push(if model.kind == ModelKind::Mlp { 1 } else { 2 });
        for v in [1, EDIT_FEATURE_VERSION, SCHEMA_VERSION, tables.width] {
            u16out(&mut out, v);
        }
        out.push(0);
        u16out(&mut out, model.thresholds.len() as u16);
        for v in &model.thresholds {
            float(&mut out, *v);
        }
        u32out(&mut out, model.support.len() as u32);
        for v in &model.support {
            u32out(&mut out, *v);
        }
        for cal in &model.calibration {
            match cal {
                Calibration::Identity => {
                    u16out(&mut out, 0);
                    u16out(&mut out, 0);
                }
                Calibration::Platt(a, b) => {
                    u16out(&mut out, 1);
                    u16out(&mut out, 0);
                    float(&mut out, *a);
                    float(&mut out, *b);
                }
                Calibration::Isotonic(pairs) => {
                    u16out(&mut out, 2);
                    u16out(&mut out, pairs.len() as u16);
                    for (x, y) in pairs {
                        float(&mut out, *x);
                        float(&mut out, *y);
                    }
                }
            }
        }
        for head in &model.heads {
            match head {
                Head::Mlp(layers) => {
                    u16out(&mut out, layers.len() as u16);
                    for layer in layers {
                        u16out(&mut out, layer.rows as u16);
                        u16out(&mut out, layer.cols as u16);
                        out.push(layer.activation);
                        for v in layer.weights.iter().chain(&layer.bias) {
                            float(&mut out, *v);
                        }
                    }
                }
                Head::Gbdt(base, trees) => {
                    u32out(&mut out, trees.len() as u32);
                    float(&mut out, *base);
                    for tree in trees {
                        u16out(&mut out, tree.len() as u16);
                        for node in tree {
                            u16out(&mut out, node.feature);
                            float(&mut out, node.threshold);
                            u16out(&mut out, node.left);
                            u16out(&mut out, node.right);
                            float(&mut out, node.value);
                        }
                    }
                }
            }
        }
        let crc = crc32(&out);
        u32out(&mut out, crc);
        out
    }
    fn random(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        // Quantized xorshift weights keep each float's low byte zero, so
        // numeric fixtures cannot accidentally encode long ASCII strings.
        ((*seed % 2049) as f32 - 1024.0) / 1024.0
    }
    fn fixture(kind: ModelKind) -> ArtifactTables {
        let mut seed = 0x17abc123;
        let mut heads = Vec::new();
        for h in 0..3 {
            heads.push(match kind {
                ModelKind::Mlp => Head::Mlp(vec![
                    Layer {
                        rows: 8,
                        cols: 248,
                        activation: 1,
                        weights: (0..8 * 248).map(|_| random(&mut seed) / 32.0).collect(),
                        bias: vec![0.125; 8],
                    },
                    Layer {
                        rows: 1,
                        cols: 8,
                        activation: 0,
                        weights: (0..8).map(|_| random(&mut seed) / 4.0).collect(),
                        bias: vec![h as f32 + 0.5],
                    },
                ]),
                ModelKind::Gbdt => Head::Gbdt(
                    h as f32 + 0.5,
                    (0..2)
                        .map(|i| {
                            vec![
                                Node {
                                    feature: if i == 0 { 0x8000 } else { 152 },
                                    threshold: 0.0,
                                    left: 1,
                                    right: 2,
                                    value: 0.0,
                                },
                                Node {
                                    feature: 0,
                                    threshold: 0.0,
                                    left: 65535,
                                    right: 65535,
                                    value: -0.125,
                                },
                                Node {
                                    feature: 0,
                                    threshold: 0.0,
                                    left: 65535,
                                    right: 65535,
                                    value: 0.25,
                                },
                            ]
                        })
                        .collect(),
                ),
            });
        }
        ArtifactTables {
            width: if kind == ModelKind::Mlp { 248 } else { 3 },
            model: Compact {
                kind,
                thresholds: vec![0.5, 0.625, 0.75, f32::NAN],
                support: (0..4).map(|code| cell_id(1, 2, 0, code)).collect(),
                calibration: vec![
                    Calibration::Platt(1.0, -0.125),
                    Calibration::Identity,
                    Calibration::Identity,
                ],
                heads,
            },
        }
    }
    // The shipped artifacts and parity vectors come from the Python trainer
    // (training/fixtures/golden/README.md); these tables only feed the guard tests.
    #[test]
    fn compact_codes_are_unique_and_ordered() {
        assert_eq!(KEEP_CODE, 0);
        let schema = Schema::v1();
        let mut codes = Vec::new();
        for key in Key::ALL {
            if schema.spec(*key).learnable {
                codes.push(edit_code(*key).unwrap());
            } else {
                assert_eq!(edit_code(*key), None);
            }
        }
        assert_eq!(codes, (1..=9).collect::<Vec<_>>());
        assert_eq!(cell_id(1, 2, 3, 4), 0x01020304);
    }
    #[test]
    fn hostile_bytes_never_load_or_panic() {
        for kind in [ModelKind::Mlp, ModelKind::Gbdt] {
            let bytes = write_artifact(&fixture(kind));
            assert!(Compact::from_bytes(&bytes).is_ok());
            for length in 0..bytes.len() {
                assert!(Compact::from_bytes(&bytes[..length]).is_err());
            }
            for i in 0..4096 {
                let mut corrupt = bytes.clone();
                let at = (i * 37) % bytes.len();
                corrupt[at] ^= 1 << (i % 8);
                assert!(Compact::from_bytes(&corrupt).is_err());
            }
        }
    }
    #[test]
    fn checksum_and_structural_guards() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
        let original = write_artifact(&fixture(ModelKind::Mlp));
        for (at, value, expected) in [
            (0, 0, ArtifactError::BadMagic),
            (5, 2, ArtifactError::Version(2)),
            (6, 3, ArtifactError::Kind(3)),
            (7, 2, ArtifactError::FeatureVersion(2)),
            (9, 2, ArtifactError::EditFeatureVersion(2)),
            (11, 1, ArtifactError::SchemaVersion(1)),
            (13, 0, ArtifactError::Width(0)),
            (15, 1, ArtifactError::Shape),
            (16, 0, ArtifactError::Shape),
        ] {
            let mut bytes = original.clone();
            bytes[at] = value;
            let end = bytes.len() - 4;
            let crc = crc32(&bytes[..end]);
            bytes[end..].copy_from_slice(&crc.to_le_bytes());
            assert_eq!(Compact::from_bytes(&bytes).err(), Some(expected));
        }
        assert_eq!(
            Compact::from_bytes(&vec![0; 2_000_001]).err(),
            Some(ArtifactError::Size(2_000_001))
        );
        let mut t = fixture(ModelKind::Mlp);
        t.model.support.reverse();
        assert_eq!(
            Compact::from_bytes(&write_artifact(&t)).err(),
            Some(ArtifactError::Shape)
        );
        t = fixture(ModelKind::Mlp);
        if let Head::Mlp(layers) = &mut t.model.heads[0] {
            layers[0].weights[0] = f32::INFINITY;
        }
        assert_eq!(
            Compact::from_bytes(&write_artifact(&t)).err(),
            Some(ArtifactError::Shape)
        );
        let mut t = fixture(ModelKind::Gbdt);
        if let Head::Gbdt(_, trees) = &mut t.model.heads[0] {
            trees[0][0].left = 0;
        }
        assert_eq!(
            Compact::from_bytes(&write_artifact(&t)).err(),
            Some(ArtifactError::Shape)
        );
    }
    #[test]
    fn malformed_tables_with_valid_checksums_are_rejected() {
        let original = write_artifact(&fixture(ModelKind::Mlp));
        // Every prefix with a repaired checksum reaches structural parsing.
        for end in (16..original.len() - 4).step_by(7) {
            let mut bytes = original[..end].to_vec();
            let crc = crc32(&bytes);
            u32out(&mut bytes, crc);
            assert!(Compact::from_bytes(&bytes).is_err());
        }
        for mutation in 0..15 {
            let mut t = fixture(ModelKind::Mlp);
            match mutation {
                0 => t.width = 257,
                1 => t.width = 249,
                2 => t.model.thresholds[0] = f32::INFINITY,
                3 => t.model.thresholds[0] = -0.1,
                4 => t.model.support[1] = t.model.support[0],
                5 => t.model.calibration[0] = Calibration::Platt(f32::NAN, 0.0),
                6 => t.model.calibration[0] = Calibration::Isotonic(vec![(0.0, 0.0)]),
                7 => t.model.calibration[0] = Calibration::Isotonic(vec![(0.0, 0.0), (0.0, 1.0)]),
                8 => t.model.calibration[0] = Calibration::Isotonic(vec![(0.0, 1.0), (1.0, 0.0)]),
                _ => {
                    if let Head::Mlp(layers) = &mut t.model.heads[0] {
                        match mutation {
                            9 => layers.clear(),
                            10 => layers[0].rows = 0,
                            11 => layers[0].rows = 257,
                            12 => layers[0].cols = 247,
                            13 => layers[0].activation = 3,
                            _ => {
                                layers.pop();
                            }
                        }
                    }
                }
            }
            assert!(
                Compact::from_bytes(&write_artifact(&t)).is_err(),
                "mutation {mutation}"
            );
        }
        for mutation in 0..6 {
            let mut t = fixture(ModelKind::Gbdt);
            if let Head::Gbdt(base, trees) = &mut t.model.heads[0] {
                match mutation {
                    0 => *base = f32::NAN,
                    1 => trees[0].clear(),
                    2 => trees[0][0].feature = 248,
                    3 => trees[0][0].right = 3,
                    4 => trees[0][0].left = 65535,
                    _ => trees[0][2].right = 0,
                }
            }
            assert!(
                Compact::from_bytes(&write_artifact(&t)).is_err(),
                "tree mutation {mutation}"
            );
        }
        let mut extra = original[..original.len() - 4].to_vec();
        extra.push(0);
        let crc = crc32(&extra);
        u32out(&mut extra, crc);
        assert_eq!(
            Compact::from_bytes(&extra).err(),
            Some(ArtifactError::Shape)
        );
        let mut t = fixture(ModelKind::Mlp);
        t.model.calibration[0] = Calibration::Isotonic(vec![(0.0, 0.0), (1.0, 1.0)]);
        if let Head::Mlp(layers) = &mut t.model.heads[0] {
            for layer in layers.iter_mut() {
                layer.weights.fill(0.0);
                layer.bias.fill(0.0);
            }
            layers[1].activation = 2;
        }
        let loaded = Compact::from_bytes(&write_artifact(&t)).unwrap();
        assert_eq!(
            loaded.score_slices(&[0.0; 152], &[0.0; 96], 0).p_success,
            0.5
        );
        assert_eq!(loaded.version(), ModelVersion(1));
    }

    #[test]
    fn support_lengths_missing_and_calibration() {
        let mut t = fixture(ModelKind::Gbdt);
        let m = &t.model;
        assert_eq!(m.score(&Input::default()).support, 0.0);
        assert_eq!(
            m.score_in_cell(&Input::default(), cell_id(1, 2, 0, 0))
                .support,
            1.0
        );
        assert_eq!(m.threshold(0), Some(0.5));
        assert_eq!(m.threshold(3), None);
        assert_eq!(m.threshold(255), None);
        for len in [0, 151, 153, 256] {
            assert!(m
                .score_slices(&vec![0.0; len], &[0.0; 96], 0)
                .p_success
                .is_nan());
        }
        for len in [0, 95, 97] {
            assert!(m
                .score_slices(&[0.0; 152], &vec![0.0; len], 0)
                .p_success
                .is_nan());
        }
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut base = [0.0; 152];
            base[0] = value;
            let left = m.score_slices(&base, &[0.0; 96], 0);
            assert!(left.p_success.is_nan());
            assert!((left.latency_ms - 1.25f32.exp_m1()).abs() < 1e-6);
            let mut edit = [0.0; 96];
            edit[0] = value;
            let right = m.score_slices(&[0.0; 152], &edit, 0);
            assert!(right.p_success.is_nan());
            assert!((right.latency_ms - 1.625f32.exp_m1()).abs() < 1e-6);
        }
        t.model.support.clear();
        assert_eq!(t.model.score(&Input::default()).support, 1.0);
        let cal = Calibration::Isotonic(vec![(0.0, 0.25), (1.0, 0.75)]);
        assert_eq!(cal.apply(-1.0), 0.25);
        assert_eq!(cal.apply(0.5), 0.5);
        assert_eq!(cal.apply(2.0), 0.75);
    }
}
