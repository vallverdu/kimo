//! Reading a device model (`~/.kimo/models/*.glb`): plain binary glTF, as
//! `scripts/device-models/build.py` writes it. The node tree is flattened into world space and
//! triangles are grouped by colour, which is all a map needs. Not a general glTF reader: no
//! compression, no textures, no animation. Pure.

use serde_json::Value;

/// All the triangles of one colour, in metres.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Part {
    pub color: [f32; 3],
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

type Mat4 = [f32; 16];
const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

fn floats<const N: usize>(v: &Value) -> Option<[f32; N]> {
    let list = v.as_array()?;
    if list.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(list) {
        *o = x.as_f64()? as f32;
    }
    Some(out)
}

/// A node's local transform (a matrix, or translation, rotation and scale).
fn local(node: &Value) -> Mat4 {
    if let Some(m) = floats::<16>(&node["matrix"]) {
        return m;
    }
    let t = floats::<3>(&node["translation"]).unwrap_or([0.0; 3]);
    let [x, y, z, w] = floats::<4>(&node["rotation"]).unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let s = floats::<3>(&node["scale"]).unwrap_or([1.0; 3]);
    [
        (1.0 - 2.0 * (y * y + z * z)) * s[0],
        (2.0 * (x * y + z * w)) * s[0],
        (2.0 * (x * z - y * w)) * s[0],
        0.0,
        (2.0 * (x * y - z * w)) * s[1],
        (1.0 - 2.0 * (x * x + z * z)) * s[1],
        (2.0 * (y * z + x * w)) * s[1],
        0.0,
        (2.0 * (x * z + y * w)) * s[2],
        (2.0 * (y * z - x * w)) * s[2],
        (1.0 - 2.0 * (x * x + y * y)) * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

fn apply_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

fn apply_normal(m: &Mat4, n: [f32; 3]) -> [f32; 3] {
    let v = [
        m[0] * n[0] + m[4] * n[1] + m[8] * n[2],
        m[1] * n[0] + m[5] * n[1] + m[9] * n[2],
        m[2] * n[0] + m[6] * n[1] + m[10] * n[2],
    ];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / len, v[1] / len, v[2] / len]
}

struct Glb<'a> {
    json: Value,
    bin: &'a [u8],
    /// Nodes visited and vertices kept so far: a damaged or hostile file (a node that is its
    /// own grandchild, one mesh placed a million times) stops here instead of eating memory.
    visits: std::cell::Cell<usize>,
    vertices: std::cell::Cell<usize>,
}

const MAX_VISITS: usize = 100_000;
const MAX_VERTICES: usize = 4_000_000;

impl Glb<'_> {
    /// An accessor's bytes: `(offset into bin, count, stride)`.
    fn accessor(&self, i: u64, item: usize) -> Result<(usize, usize, usize), String> {
        let a = &self.json["accessors"][i as usize];
        if a["sparse"].is_object() {
            return Err("sparse accessors are not supported".into());
        }
        let view = &self.json["bufferViews"][a["bufferView"].as_u64().unwrap_or(0) as usize];
        let offset = (view["byteOffset"].as_u64().unwrap_or(0) as usize)
            .checked_add(a["byteOffset"].as_u64().unwrap_or(0) as usize)
            .ok_or("an accessor overflows")?;
        let count = a["count"].as_u64().ok_or("an accessor has no count")? as usize;
        let stride = view["byteStride"].as_u64().map_or(item, |s| s as usize);
        if stride < item {
            return Err("an accessor's items overlap".into());
        }
        let end = count
            .checked_mul(stride)
            .and_then(|n| n.checked_add(offset))
            .ok_or("an accessor overflows")?;
        if count > 0 && end - stride + item > self.bin.len() {
            return Err("an accessor runs past the end of the file".into());
        }
        Ok((offset, count, stride))
    }

    fn vec3(&self, i: u64) -> Result<Vec<[f32; 3]>, String> {
        let a = &self.json["accessors"][i as usize];
        if a["componentType"] != 5126 || a["type"] != "VEC3" {
            return Err("positions and normals must be floats".into());
        }
        let (offset, count, stride) = self.accessor(i, 12)?;
        Ok((0..count)
            .map(|n| {
                let at = offset + n * stride;
                let f = |k: usize| {
                    f32::from_le_bytes(self.bin[at + k * 4..at + k * 4 + 4].try_into().unwrap())
                };
                [f(0), f(1), f(2)]
            })
            .collect())
    }

    fn indices(&self, i: u64) -> Result<Vec<u32>, String> {
        let a = &self.json["accessors"][i as usize];
        let size = match a["componentType"].as_u64() {
            Some(5121) => 1,
            Some(5123) => 2,
            Some(5125) => 4,
            _ => return Err("indices must be unsigned integers".into()),
        };
        let (offset, count, stride) = self.accessor(i, size)?;
        Ok((0..count)
            .map(|n| {
                let at = offset + n * stride;
                match size {
                    1 => u32::from(self.bin[at]),
                    2 => u32::from(u16::from_le_bytes(self.bin[at..at + 2].try_into().unwrap())),
                    _ => u32::from_le_bytes(self.bin[at..at + 4].try_into().unwrap()),
                }
            })
            .collect())
    }

    fn color(&self, primitive: &Value) -> [f32; 3] {
        primitive["material"]
            .as_u64()
            .and_then(|m| {
                floats::<4>(
                    &self.json["materials"][m as usize]["pbrMetallicRoughness"]["baseColorFactor"],
                )
            })
            .map_or([0.55, 0.55, 0.57], |c| [c[0], c[1], c[2]])
    }

    fn walk(
        &self,
        node: u64,
        parent: &Mat4,
        parts: &mut Vec<Part>,
        depth: u32,
    ) -> Result<(), String> {
        if depth > 64 {
            return Err("the node tree is too deep".into());
        }
        self.visits.set(self.visits.get() + 1);
        if self.visits.get() > MAX_VISITS {
            return Err("too many nodes".into());
        }
        let n = &self.json["nodes"][node as usize];
        let world = multiply(parent, &local(n));
        if let Some(mesh) = n["mesh"].as_u64() {
            for p in self.json["meshes"][mesh as usize]["primitives"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if p["mode"].as_u64().unwrap_or(4) != 4 {
                    continue;
                }
                let Some(pos) = p["attributes"]["POSITION"].as_u64() else {
                    continue;
                };
                let positions = self.vec3(pos)?;
                self.vertices.set(self.vertices.get() + positions.len());
                if self.vertices.get() > MAX_VERTICES {
                    return Err("too many vertices for a map model".into());
                }
                let normals = match p["attributes"]["NORMAL"].as_u64() {
                    Some(i) => self.vec3(i)?,
                    None => Vec::new(),
                };
                let indices = match p["indices"].as_u64() {
                    Some(i) => self.indices(i)?,
                    None => (0..positions.len() as u32).collect(),
                };
                if indices.iter().any(|i| *i as usize >= positions.len()) {
                    return Err("an index points past the vertices".into());
                }
                let color = self.color(p);
                let at = parts.iter().position(|x| x.color == color);
                let part = match at {
                    Some(i) => &mut parts[i],
                    None => {
                        parts.push(Part {
                            color,
                            ..Default::default()
                        });
                        parts.last_mut().unwrap()
                    }
                };
                let base = part.positions.len() as u32;
                part.indices.extend(indices.iter().map(|i| i + base));
                let flat = [0.0, 1.0, 0.0];
                for (k, v) in positions.iter().enumerate() {
                    part.positions.push(apply_point(&world, *v));
                    part.normals.push(apply_normal(
                        &world,
                        normals.get(k).copied().unwrap_or(flat),
                    ));
                }
            }
        }
        for child in n["children"].as_array().into_iter().flatten() {
            self.walk(
                child.as_u64().ok_or("a bad child node")?,
                &world,
                parts,
                depth + 1,
            )?;
        }
        Ok(())
    }
}

/// The triangles of a `.glb` file, in world space, grouped by colour.
pub fn parse(file: &[u8]) -> Result<Vec<Part>, String> {
    if file.len() < 20 || &file[..4] != b"glTF" {
        return Err("not a GLB file".into());
    }
    let word = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
    let json_len = word(12);
    if &file[16..20] != b"JSON" || 20 + json_len > file.len() {
        return Err("a damaged GLB file".into());
    }
    let json: Value =
        serde_json::from_slice(&file[20..20 + json_len]).map_err(|e| format!("its JSON: {e}"))?;
    let at = 20 + json_len;
    let bin = if at + 8 <= file.len() && &file[at + 4..at + 8] == b"BIN\0" {
        let len = word(at);
        file.get(at + 8..at + 8 + len).ok_or("a damaged GLB file")?
    } else {
        &[]
    };
    let glb = Glb {
        json,
        bin,
        visits: Default::default(),
        vertices: Default::default(),
    };
    let scene = glb.json["scene"].as_u64().unwrap_or(0) as usize;
    let mut parts = Vec::new();
    for node in glb.json["scenes"][scene]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        glb.walk(
            node.as_u64().ok_or("a bad scene node")?,
            &IDENTITY,
            &mut parts,
            0,
        )?;
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One triangle under a node moved up by 2 and turned 90° about y, in a real GLB's layout.
    fn triangle() -> Vec<u8> {
        triangle_with(|_| {})
    }

    /// The same, with its JSON changed by `edit`.
    fn triangle_with(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut bin = Vec::new();
        for f in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bin.extend(f.to_le_bytes());
        }
        for f in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0] {
            bin.extend(f.to_le_bytes());
        }
        for i in [0u16, 1, 2, 0] {
            bin.extend(i.to_le_bytes());
        }
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let mut json = serde_json::json!({
            "scene": 0,
            "scenes": [{ "nodes": [0] }],
            "nodes": [
                { "translation": [0.0, 2.0, 0.0], "children": [1] },
                { "mesh": 0, "rotation": [0.0, half, 0.0, half] },
            ],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0, "NORMAL": 1 }, "indices": 2, "material": 0 }] }],
            "materials": [{ "pbrMetallicRoughness": { "baseColorFactor": [0.1, 0.2, 0.3, 1.0] } }],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
                { "buffer": 0, "byteOffset": 36, "byteLength": 36 },
                { "buffer": 0, "byteOffset": 72, "byteLength": 6 },
            ],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" },
                { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" },
                { "bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR" },
            ],
        });
        edit(&mut json);
        let mut text = serde_json::to_vec(&json).unwrap();
        while !text.len().is_multiple_of(4) {
            text.push(b' ');
        }
        let total = 12 + 8 + text.len() + 8 + bin.len();
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((text.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(&text);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(&bin);
        out
    }

    #[test]
    fn nodes_are_flattened_into_world_space() {
        let parts = parse(&triangle()).unwrap();
        assert_eq!(parts.len(), 1);
        let p = &parts[0];
        assert_eq!(p.color, [0.1, 0.2, 0.3]);
        assert_eq!(p.indices, [0, 1, 2]);
        // (1,0,0) turned 90° about y is (0,0,-1), then up by 2.
        let v = p.positions[1];
        assert!(
            v[0].abs() < 1e-5 && (v[1] - 2.0).abs() < 1e-5 && (v[2] + 1.0).abs() < 1e-5,
            "{v:?}"
        );
        let n = p.normals[0];
        assert!(
            (n[0] - 1.0).abs() < 1e-5 && n[1].abs() < 1e-5 && n[2].abs() < 1e-5,
            "{n:?}"
        );
    }

    #[test]
    fn damaged_files_are_refused_not_panicked_on() {
        assert!(parse(b"nope").is_err());
        assert!(parse(&[0u8; 40]).is_err());
        let good = triangle();
        for cut in [13, 21, good.len() - 4, good.len() - 20] {
            let _ = parse(&good[..cut]);
        }
        // An accessor pointing past the data.
        let mut text = String::from_utf8_lossy(&good[20..]).to_string();
        text = text.replace("\"count\":3", "\"count\":3000000");
        let mut bad = good[..20].to_vec();
        bad.extend(text.as_bytes());
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn hostile_files_stop_early() {
        // A node that is its own child, twice: without a budget it would never end.
        let looped = triangle_with(|j| j["nodes"][0]["children"] = serde_json::json!([0, 0]));
        assert!(parse(&looped).is_err());
        // Items that overlap (stride 0) with a huge count: refused before any allocation.
        let overlap = triangle_with(|j| {
            j["bufferViews"][0]["byteStride"] = 0.into();
            j["accessors"][0]["count"] = 4_000_000_000u64.into();
        });
        assert!(parse(&overlap).is_err());
        // One mesh placed many times.
        let many = triangle_with(|j| {
            j["nodes"][0]["children"] = serde_json::json!(vec![1; 60_000]);
        });
        assert!(parse(&many).is_ok_and(|p| p[0].positions.len() == 180_000));
        let too_many = triangle_with(|j| {
            j["nodes"] = serde_json::json!([
                { "children": vec![1; 1000] },
                { "children": vec![2; 99] },
                { "mesh": 0 },
            ]);
        });
        assert!(parse(&too_many).is_err());
    }
}
