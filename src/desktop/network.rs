//! The Network view: every machine in 3D, to scale, drawn from its CAD model when this kimo holds
//! one (`~/.kimo/models`, the same files the web map reads), with who reaches whom over SSH, its
//! power and its plug. A client of the API like the rest of the window: tailnet from
//! `/network/tailnet`, the rest from the fleet coordinator at `/fleet` when there is one.

use super::theme;
use crate::client::Client;
use crate::core::glb::{self, Part};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, UiBuilder, Vec2};
use eframe::glow::{self, HasContext};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_secs(5);

// ----- what is known about the machines (pure) ----------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Machine {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub role: String,
    pub zone: String,
    pub zerotier: String,
    pub tailscale: String,
    pub alias: String,
    pub plug: Value,
    pub can_wake: bool,
    pub added: bool,
    pub in_fleet: bool,
    pub hub: bool,
    pub facts: Value,
    pub error: String,
    /// Where it stands on the floor, in metres.
    pub at: [f32; 2],
}

/// The fleet's hardware models, as the web map draws them.
pub fn kind_of(model: &str, name: &str, os: &str) -> &'static str {
    match model {
        "nuc-11-extreme" | "linux-gpu-pc" => return "nucExtreme",
        "nuc-11-enthusiast" | "linux-pc" => return "nucEnthusiast",
        "nuc-13-extreme" => return "nuc13Extreme",
        "gaming-laptop" => return "laptop",
        "rpi-zero-2w" => return "zero2",
        "jetson-orin-nano" => return "jetson",
        "rpi5" | "rpi5-hailo" => return "rpi5",
        "rpi4" => return "rpi4",
        "macbook-air" | "mac" => return "macbookAir",
        "mac-mini" => return "macMini",
        _ => {}
    }
    let n = name.to_ascii_lowercase();
    let has = |s: &str| n.contains(s);
    match os.to_ascii_lowercase().as_str() {
        "ios" => return if has("ipad") { "tablet" } else { "phone" },
        "android" => {
            return if has("tab") || has("pad") {
                "tablet"
            } else {
                "phone"
            }
        }
        "macos" => return if has("mini") { "macMini" } else { "macbookAir" },
        "windows" => return "laptop",
        _ => {}
    }
    if has("jetson") || has("orin") {
        "jetson"
    } else if has("zero") {
        "zero2"
    } else if has("rpi5") || has("pi5") {
        "rpi5"
    } else if has("rpi") || has("raspberry") {
        "rpi4"
    } else if has("katana") || has("laptop") {
        "laptop"
    } else if has("extreme") || has("nuc11bt") {
        "nucExtreme"
    } else {
        "nucEnthusiast"
    }
}

fn zone_of(kind: &str, labels: &[String], hub: bool) -> String {
    if let Some(site) = labels.iter().find_map(|l| l.strip_prefix("site:")) {
        return format!("site:{site}");
    }
    if hub {
        return "core".into();
    }
    match kind {
        "phone" | "tablet" => "screens",
        "rpi4" | "rpi5" | "zero2" | "jetson" => "boards",
        "macbookAir" | "macMini" => "apple",
        _ => "compute",
    }
    .into()
}

/// The tailnet and the fleet, one entry per machine.
pub fn merge(tailnet: &Value, fleet: &Value, models: &[String]) -> Vec<Machine> {
    let _ = models;
    let mut machines: BTreeMap<String, Machine> = BTreeMap::new();
    for d in tailnet["devices"].as_array().into_iter().flatten() {
        if d["mine"] != true {
            continue;
        }
        let name = d["name"].as_str().unwrap_or_default().to_string();
        let os = d["os"].as_str().unwrap_or_default();
        let host = d["host_name"].as_str().unwrap_or_default();
        machines.insert(
            name.clone(),
            Machine {
                id: name.clone(),
                kind: kind_of("", &format!("{name} {host}"), os).into(),
                status: if d["online"] == true { "online" } else { "off" }.into(),
                tailscale: d["ips"][0].as_str().unwrap_or_default().to_string(),
                hub: d["self"] == true,
                ..Default::default()
            },
        );
    }
    for f in fleet.as_array().into_iter().flatten() {
        let id = f["id"].as_str().unwrap_or_default().to_string();
        let r = &f["registry"];
        let ts = r["tailscale"].as_str().unwrap_or_default();
        // The same machine on both: under its Tailscale name, or its own id.
        let key = if machines.contains_key(ts) {
            ts.to_string()
        } else {
            id.clone()
        };
        let m = machines.entry(key).or_insert_with(|| Machine {
            id: id.clone(),
            tailscale: ts.to_string(),
            ..Default::default()
        });
        let name = m.id.clone();
        m.kind = kind_of(r["model"].as_str().unwrap_or_default(), &name, "").into();
        m.role = r["role"].as_str().unwrap_or_default().replace('-', " ");
        m.zerotier = r["zerotier"].as_str().unwrap_or_default().to_string();
        m.alias = r["alias"].as_str().map(str::to_string).unwrap_or_else(|| {
            if r["user"].is_string() {
                format!("z{id}")
            } else {
                String::new()
            }
        });
        m.plug = f["plug"].clone();
        m.can_wake = f["can_wake"] == true;
        m.added = f["added"] == true;
        m.in_fleet = true;
        m.facts = f["facts"].clone();
        m.error = f["error"].as_str().unwrap_or_default().to_string();
        let state = f["state"].as_str().unwrap_or_default();
        m.status = match state {
            "ready" | "idle" | "manual" => "online",
            "busy" => "busy",
            "waking" | "booted" | "draining" => "waking",
            "unreachable" => "unreachable",
            "off" => {
                if m.status == "online" {
                    "online"
                } else {
                    "off"
                }
            }
            _ if m.status.is_empty() => "off",
            _ => m.status.as_str(),
        }
        .to_string();
        let labels: Vec<String> = f["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str().map(str::to_string))
            .collect();
        m.zone = zone_of(&m.kind, &labels, m.hub);
    }
    let mut list: Vec<Machine> = machines.into_values().collect();
    for m in &mut list {
        if m.zone.is_empty() {
            m.zone = zone_of(&m.kind, &[], m.hub);
        }
    }
    if !list.iter().any(|m| m.hub) {
        if let Some(m) = list.iter_mut().find(|m| m.kind == "macbookAir") {
            m.zone = "core".into();
        }
    }
    layout(&mut list);
    list
}

/// Zones on the floor, as the web map lays them out.
pub fn layout(list: &mut [Machine]) {
    let mut zones: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, m) in list.iter().enumerate() {
        zones.entry(m.zone.clone()).or_default().push(i);
    }
    let mut site = 0.0;
    for (zone, members) in zones {
        let (at, cols, step): ([f32; 2], usize, f32) = match zone.as_str() {
            "core" => ([0.0, 0.0], 3, 0.9),
            "compute" => ([0.0, -1.5], 3, 1.0),
            "boards" => ([1.9, 0.1], 3, 0.6),
            "apple" => ([-1.9, 0.1], 3, 0.9),
            "screens" => ([0.0, 1.4], 4, 0.45),
            _ => {
                site += 1.0;
                ([2.2 + site * 1.5, -1.3], 2, 0.8)
            }
        };
        let rows = members.len().div_ceil(cols);
        let width = members.len().min(cols);
        for (n, i) in members.iter().enumerate() {
            let (c, r) = ((n % cols) as f32, (n / cols) as f32);
            list[*i].at = [
                at[0] + (c - (width as f32 - 1.0) / 2.0) * step,
                at[1] + (r - (rows as f32 - 1.0) / 2.0) * step,
            ];
        }
    }
}

/// A pair of machines, from the mesh check. See `meshPairs` in web/js/map-data.js.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub a: String,
    pub b: String,
    pub state: &'static str,
}

pub fn mesh_pairs(report: &Value) -> Vec<Pair> {
    let mut pairs: BTreeMap<(String, String), Vec<&Value>> = BTreeMap::new();
    for l in report["links"].as_array().into_iter().flatten() {
        let (from, to) = (
            l["from"].as_str().unwrap_or(""),
            l["to"].as_str().unwrap_or(""),
        );
        let key = if from < to { (from, to) } else { (to, from) };
        pairs
            .entry((key.0.to_string(), key.1.to_string()))
            .or_default()
            .push(l);
    }
    pairs
        .into_iter()
        .map(|((a, b), links)| {
            // Tailscale SSH asking for a browser check is not a broken link.
            let judged: Vec<&&Value> = links.iter().filter(|l| l["found"] != "check").collect();
            let seen: Vec<&&Value> = if judged.is_empty() {
                links.iter().collect()
            } else {
                judged
            };
            let state = if seen
                .iter()
                .any(|l| l["expect"] == "password" && l["found"] == "ok")
            {
                "violation"
            } else if seen.iter().any(|l| l["as_intended"] != true) {
                "problem"
            } else if seen.iter().any(|l| l["expect"] == "password") {
                "oneway"
            } else {
                "good"
            };
            Pair { a, b, state }
        })
        .collect()
}

/// One line about a plug: `on, 41 W, 120 Wh today`.
pub fn plug_line(plug: &Value) -> String {
    if !plug.is_object() {
        return String::new();
    }
    let s = &plug["status"];
    if !s.is_object() {
        return plug["error"].as_str().unwrap_or("not read yet").to_string();
    }
    let mut parts = vec![if s["on"] == true {
        "on".to_string()
    } else {
        "off".to_string()
    }];
    if let Some(w) = s["watts"].as_f64() {
        parts.push(format!("{w} W"));
    }
    if let Some(wh) = s["today_wh"].as_f64() {
        parts.push(format!("{wh} Wh today"));
    }
    parts.join(", ")
}

// ----- the camera (pure) ----------------------------------------------------------------------

type Mat = [f32; 16];

fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: 0.25,
            pitch: 0.75,
            distance: 7.5,
            target: [0.6, 0.0, 0.0],
        }
    }
}

impl Camera {
    pub fn eye(&self) -> [f32; 3] {
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        [
            self.target[0] + self.distance * cp * self.yaw.sin(),
            self.target[1] + self.distance * sp,
            self.target[2] + self.distance * cp * self.yaw.cos(),
        ]
    }

    /// Projection times view, for a view of this aspect ratio.
    pub fn matrix(&self, aspect: f32) -> Mat {
        let f = 1.0 / (30f32.to_radians() / 2.0).tan();
        let (near, far) = (0.02, 200.0);
        let projection: Mat = [
            f / aspect.max(0.01),
            0.0,
            0.0,
            0.0,
            0.0,
            f,
            0.0,
            0.0,
            0.0,
            0.0,
            (far + near) / (near - far),
            -1.0,
            0.0,
            0.0,
            2.0 * far * near / (near - far),
            0.0,
        ];
        let eye = self.eye();
        let forward = norm([
            self.target[0] - eye[0],
            self.target[1] - eye[1],
            self.target[2] - eye[2],
        ]);
        let right = norm(cross(forward, [0.0, 1.0, 0.0]));
        let up = cross(right, forward);
        let view: Mat = [
            right[0],
            up[0],
            -forward[0],
            0.0,
            right[1],
            up[1],
            -forward[1],
            0.0,
            right[2],
            up[2],
            -forward[2],
            0.0,
            -dot(right, eye),
            -dot(up, eye),
            dot(forward, eye),
            1.0,
        ];
        mul(&projection, &view)
    }

    /// Where a point lands in a rectangle of the screen; `None` behind the camera.
    pub fn project(&self, p: [f32; 3], rect: Rect) -> Option<Pos2> {
        let m = self.matrix(rect.width() / rect.height().max(1.0));
        let w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
        if w <= 0.001 {
            return None;
        }
        let x = (m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12]) / w;
        let y = (m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13]) / w;
        Some(Pos2::new(
            rect.left() + (x + 1.0) / 2.0 * rect.width(),
            rect.top() + (1.0 - y) / 2.0 * rect.height(),
        ))
    }

    /// The closest view from this angle where every point fits in the rectangle.
    pub fn fit(&mut self, points: &[[f32; 3]], rect: Rect) {
        if points.is_empty() {
            return;
        }
        let min = |i: usize| points.iter().map(|p| p[i]).fold(f32::MAX, f32::min);
        let max = |i: usize| points.iter().map(|p| p[i]).fold(f32::MIN, f32::max);
        self.target = [(min(0) + max(0)) / 2.0, 0.0, (min(2) + max(2)) / 2.0];
        let inner = rect.shrink2(Vec2::new(rect.width() * 0.08, rect.height() * 0.1));
        let (mut near, mut far) = (0.5f32, 60.0f32);
        for _ in 0..24 {
            self.distance = (near + far) / 2.0;
            let fits = points
                .iter()
                .all(|p| self.project(*p, rect).is_some_and(|q| inner.contains(q)));
            if fits {
                far = self.distance;
            } else {
                near = self.distance;
            }
        }
        self.distance = far;
    }

    /// Move the point looked at, by a drag in pixels.
    pub fn pan(&mut self, delta: Vec2) {
        let eye = self.eye();
        let forward = norm([self.target[0] - eye[0], 0.0, self.target[2] - eye[2]]);
        let right = norm(cross(forward, [0.0, 1.0, 0.0]));
        let scale = self.distance * 0.0012;
        for i in [0, 2] {
            self.target[i] += (-delta.x * right[i] + delta.y * forward[i]) * scale;
        }
    }
}

/// Stack labels so none covers another; `(anchor, size)` → how far to raise each.
pub fn declutter(tags: &[(Pos2, Vec2)]) -> Vec<f32> {
    let mut order: Vec<usize> = (0..tags.len()).collect();
    order.sort_by(|a, b| tags[*b].0.y.total_cmp(&tags[*a].0.y));
    let mut placed: Vec<Rect> = Vec::new();
    let mut lifts = vec![0.0; tags.len()];
    for i in order {
        let (at, size) = tags[i];
        let mut lift = 0.0;
        for _ in 0..30 {
            let r = Rect::from_center_size(at - Vec2::new(0.0, lift), size);
            match placed.iter().find(|p| p.expand(2.0).intersects(r)) {
                Some(p) => lift = at.y - p.top() + size.y / 2.0 + 3.0,
                None => break,
            }
        }
        placed.push(Rect::from_center_size(at - Vec2::new(0.0, lift), size));
        lifts[i] = lift;
    }
    lifts
}

// ----- drawing --------------------------------------------------------------------------------

struct GlPart {
    vao: glow::VertexArray,
    count: i32,
    color: [f32; 3],
}

struct GlScene {
    program: glow::Program,
    mvp: Option<glow::UniformLocation>,
    color: Option<glow::UniformLocation>,
    lit: Option<glow::UniformLocation>,
    models: HashMap<String, Vec<GlPart>>,
    cube: GlPart,
    lines: glow::VertexArray,
    lines_vbo: glow::Buffer,
}

const VERTEX: &str = r#"
layout(location = 0) in vec3 a_position;
layout(location = 1) in vec3 a_normal;
uniform mat4 u_mvp;
out vec3 v_normal;
void main() {
    v_normal = a_normal;
    gl_Position = u_mvp * vec4(a_position, 1.0);
}
"#;

const FRAGMENT: &str = r#"
in vec3 v_normal;
uniform vec3 u_color;
uniform float u_lit;
out vec4 out_color;
void main() {
    vec3 n = normalize(v_normal);
    float light = 0.45 + 0.55 * max(dot(n, normalize(vec3(0.35, 0.85, 0.4))), 0.0)
                + 0.12 * max(dot(n, normalize(vec3(-0.5, 0.3, -0.6))), 0.0);
    // A little of its own: black cases stay visible on a dark floor.
    out_color = vec4(mix(u_color, u_color * light + vec3(0.075) * light, u_lit), 1.0);
}
"#;

fn upload(
    gl: &glow::Context,
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    indices: &[u32],
    color: [f32; 3],
) -> Option<GlPart> {
    unsafe {
        let vao = gl.create_vertex_array().ok()?;
        gl.bind_vertex_array(Some(vao));
        let vbo = gl.create_buffer().ok()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        let mut data: Vec<f32> = Vec::with_capacity(positions.len() * 6);
        for (p, n) in positions.iter().zip(normals) {
            data.extend_from_slice(p);
            data.extend_from_slice(n);
        }
        let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 24, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, 24, 12);
        let ebo = gl.create_buffer().ok()?;
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
        let bytes = std::slice::from_raw_parts(indices.as_ptr() as *const u8, indices.len() * 4);
        gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
        gl.bind_vertex_array(None);
        Some(GlPart {
            vao,
            count: indices.len() as i32,
            color,
        })
    }
}

/// A unit box standing on y = 0, centred on x/z: machines without a model, and status lights.
fn unit_cube(gl: &glow::Context) -> Option<GlPart> {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    for (n, u, v) in faces {
        let base = positions.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = [
                (n[0] + a * u[0] + b * v[0]) * 0.5,
                (n[1] + a * u[1] + b * v[1]) * 0.5 + 0.5,
                (n[2] + a * u[2] + b * v[2]) * 0.5,
            ];
            positions.push(p);
            normals.push(n);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    upload(gl, &positions, &normals, &indices, [0.45, 0.45, 0.48])
}

fn compile(gl: &glow::Context) -> Option<glow::Program> {
    let header = if gl.version().is_embedded {
        "#version 300 es\nprecision mediump float;\n"
    } else {
        "#version 330 core\n"
    };
    unsafe {
        let program = gl.create_program().ok()?;
        let mut shaders = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, VERTEX),
            (glow::FRAGMENT_SHADER, FRAGMENT),
        ] {
            let s = gl.create_shader(kind).ok()?;
            gl.shader_source(s, &format!("{header}{source}"));
            gl.compile_shader(s);
            if !gl.get_shader_compile_status(s) {
                tracing::warn!("network view shader: {}", gl.get_shader_info_log(s));
                return None;
            }
            gl.attach_shader(program, s);
            shaders.push(s);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            tracing::warn!("network view shader: {}", gl.get_program_info_log(program));
            return None;
        }
        for s in shaders {
            gl.detach_shader(program, s);
            gl.delete_shader(s);
        }
        Some(program)
    }
}

impl GlScene {
    fn new(gl: &glow::Context) -> Option<Self> {
        let program = compile(gl)?;
        let cube = unit_cube(gl)?;
        unsafe {
            let lines = gl.create_vertex_array().ok()?;
            let lines_vbo = gl.create_buffer().ok()?;
            gl.bind_vertex_array(Some(lines));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(lines_vbo));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 12, 0);
            gl.bind_vertex_array(None);
            Some(Self {
                mvp: gl.get_uniform_location(program, "u_mvp"),
                color: gl.get_uniform_location(program, "u_color"),
                lit: gl.get_uniform_location(program, "u_lit"),
                program,
                models: HashMap::new(),
                cube,
                lines,
                lines_vbo,
            })
        }
    }

    fn draw_lines(&self, gl: &glow::Context, points: &[[f32; 3]], color: [f32; 3]) {
        if points.is_empty() {
            return;
        }
        unsafe {
            gl.bind_vertex_array(Some(self.lines));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.lines_vbo));
            let bytes = std::slice::from_raw_parts(points.as_ptr() as *const u8, points.len() * 12);
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
            gl.uniform_3_f32_slice(self.color.as_ref(), &color);
            gl.draw_arrays(glow::LINES, 0, points.len() as i32);
        }
    }

    fn draw_part(&self, gl: &glow::Context, part: &GlPart, mvp: &Mat, color: [f32; 3], lit: f32) {
        unsafe {
            gl.uniform_matrix_4_f32_slice(self.mvp.as_ref(), false, mvp);
            gl.uniform_3_f32_slice(self.color.as_ref(), &color);
            gl.uniform_1_f32(self.lit.as_ref(), lit);
            gl.bind_vertex_array(Some(part.vao));
            gl.draw_elements(glow::TRIANGLES, part.count, glow::UNSIGNED_INT, 0);
        }
    }
}

fn rgb(c: Color32) -> [f32; 3] {
    [
        f32::from(c.r()) / 255.0,
        f32::from(c.g()) / 255.0,
        f32::from(c.b()) / 255.0,
    ]
}

/// A size for machines with no CAD model, in metres (width, height, depth).
fn stand_in(kind: &str) -> [f32; 3] {
    match kind {
        "laptop" => [0.36, 0.025, 0.26],
        "macbookAir" => [0.30, 0.012, 0.215],
        "macMini" => [0.197, 0.036, 0.197],
        "phone" => [0.075, 0.009, 0.16],
        "tablet" => [0.135, 0.007, 0.195],
        "rpi4" => [0.085, 0.017, 0.056],
        "rpi5" => [0.085, 0.019, 0.056],
        "zero2" => [0.065, 0.005, 0.03],
        "jetson" => [0.1, 0.035, 0.08],
        "nucExtreme" => [0.12, 0.19, 0.357],
        "nuc13Extreme" => [0.13, 0.32, 0.35],
        _ => [0.227, 0.042, 0.146],
    }
}

fn scale_at(x: f32, z: f32, s: [f32; 3], y: f32) -> Mat {
    [
        s[0], 0.0, 0.0, 0.0, 0.0, s[1], 0.0, 0.0, 0.0, 0.0, s[2], 0.0, x, y, z, 1.0,
    ]
}

/// What one frame draws: a copy, handed to the paint callback.
struct Frame {
    camera: Camera,
    aspect: f32,
    machines: Vec<(String, String, [f32; 2], Color32)>,
    lines: Vec<(Vec<[f32; 3]>, Color32)>,
    grid: Color32,
    new_models: Vec<(String, Arc<Vec<Part>>)>,
    background: Color32,
}

// ----- the view --------------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    machines: Vec<Machine>,
    mesh: Value,
    fleet: bool,
    credential: bool,
    note: Option<String>,
    models: HashMap<String, Arc<Vec<Part>>>,
    model_files: HashMap<String, String>,
    fetching: bool,
    found: Option<Value>,
    refused: Option<(String, String, String)>,
}

pub struct NetworkView {
    shared: Arc<Mutex<Shared>>,
    last: Option<Instant>,
    camera: Camera,
    selected: Option<String>,
    show_mesh: bool,
    /// The camera was framed for this many machines.
    framed: usize,
    /// An action waiting for a yes: (machine, action, what to say, forced).
    ask: Option<(String, String, String, bool)>,
    gl: Arc<Mutex<Option<GlScene>>>,
    uploaded: std::collections::HashSet<String>,
    password: String,
}

impl Default for NetworkView {
    fn default() -> Self {
        Self {
            shared: Arc::default(),
            last: None,
            camera: Camera::default(),
            selected: None,
            show_mesh: true,
            framed: 0,
            ask: None,
            gl: Arc::default(),
            uploaded: Default::default(),
            password: String::new(),
        }
    }
}

fn top_of(kind: &str, models: &HashMap<String, Arc<Vec<Part>>>) -> f32 {
    models
        .get(kind)
        .map(|parts| {
            parts
                .iter()
                .flat_map(|p| p.positions.iter().map(|v| v[1]))
                .fold(0.0, f32::max)
        })
        .unwrap_or_else(|| stand_in(kind)[1])
}

impl NetworkView {
    /// Look again now (after opening the view or acting).
    pub fn refresh(&mut self) {
        self.last = None;
    }

    fn fetch(&mut self, client: &Client, ctx: &egui::Context) {
        if self.last.is_some_and(|t| t.elapsed() < POLL) {
            return;
        }
        {
            let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if s.fetching {
                return;
            }
            s.fetching = true;
        }
        self.last = Some(Instant::now());
        let (client, shared, ctx) = (client.clone(), self.shared.clone(), ctx.clone());
        std::thread::spawn(move || {
            let tailnet: Value = client.get("/network/tailnet").unwrap_or(Value::Null);
            let fleet: Option<Value> = client.call_timeout("GET", "/fleet/devices", None, 4).ok();
            let mesh: Value = client
                .call_timeout("GET", "/fleet/mesh", None, 4)
                .unwrap_or(Value::Null);
            let status: Value = client
                .call_timeout("GET", "/fleet/status", None, 4)
                .unwrap_or(Value::Null);
            let index: Value = client.get("/models").unwrap_or(Value::Null);
            let list: Vec<Value> = index["models"].as_array().cloned().unwrap_or_default();
            let machines = merge(&tailnet, fleet.as_ref().unwrap_or(&Value::Null), &[]);
            let wanted: Vec<(String, String)> = {
                let s = shared.lock().unwrap_or_else(|e| e.into_inner());
                list.iter()
                    .filter_map(|m| {
                        Some((
                            m["kind"].as_str()?.to_string(),
                            m["file"].as_str()?.to_string(),
                        ))
                    })
                    .filter(|(k, _)| !s.models.contains_key(k) && !s.model_files.contains_key(k))
                    .filter(|(k, _)| machines.iter().any(|m| m.kind == *k))
                    .collect()
            };
            let mut loaded = Vec::new();
            for (kind, file) in wanted {
                let parts = client
                    .raw("GET", &format!("/models/{file}"), vec![], None, 30)
                    .ok()
                    .filter(|(status, _, _)| *status == 200)
                    .and_then(|(_, _, bytes)| glb::parse(&bytes).ok());
                loaded.push((kind, file, parts));
            }
            {
                let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                s.machines = machines;
                s.mesh = mesh;
                s.fleet = fleet.is_some();
                s.credential = status["plug_credential"] == true;
                s.fetching = false;
                for (kind, file, parts) in loaded {
                    s.model_files.insert(kind.clone(), file);
                    if let Some(parts) = parts {
                        s.models.insert(kind, Arc::new(parts));
                    }
                }
            }
            ctx.request_repaint();
        });
    }

    /// Run a call on the coordinator off the window's thread; its result lands in `note`, or as
    /// a refusal a person can overrule.
    fn act(&mut self, client: &Client, ctx: &egui::Context, id: &str, action: &str, force: bool) {
        let (client, shared, ctx) = (client.clone(), self.shared.clone(), ctx.clone());
        let (id, action) = (id.to_string(), action.to_string());
        self.last = None;
        std::thread::spawn(move || {
            // `Err((text, overrule))`: the coordinator's 409 means `force` would allow it.
            let result: Result<(), (String, bool)> = match action.as_str() {
                "wake" => client
                    .post(&format!("/fleet/devices/{id}/wake"), json!({}))
                    .map(|_| ())
                    .map_err(|e| (e.to_string(), false)),
                "remove" => client
                    .delete(&format!("/fleet/devices/{id}"))
                    .map(|_| ())
                    .map_err(|e| (e.to_string(), false)),
                plug => {
                    let body = json!({ "action": plug, "force": force }).to_string();
                    match client.raw(
                        "POST",
                        &format!("/fleet/devices/{id}/plug"),
                        body.into_bytes(),
                        Some("application/json"),
                        60,
                    ) {
                        Ok((status, _, _)) if (200..300).contains(&status) => Ok(()),
                        Ok((status, _, bytes)) => {
                            let answer: Value = serde_json::from_slice(&bytes).unwrap_or_default();
                            let text = answer["error"].as_str().map_or_else(
                                || format!("the coordinator answered {status}"),
                                String::from,
                            );
                            Err((text, status == 409))
                        }
                        Err(e) => Err((e.to_string(), false)),
                    }
                }
            };
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Ok(()) => s.note = Some(format!("{id}: {action} done")),
                Err((text, true)) => s.refused = Some((id, action, text)),
                Err((text, false)) => s.note = Some(text),
            }
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut Ui, client: &Client) {
        let ctx = ui.ctx().clone();
        self.fetch(client, &ctx);
        ctx.request_repaint_after(POLL);

        let (machines, mesh, fleet, credential, note, models, refused, found) = {
            let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            (
                s.machines.clone(),
                s.mesh.clone(),
                s.fleet,
                s.credential,
                s.note.clone(),
                s.models.clone(),
                s.refused.take(),
                s.found.clone(),
            )
        };
        if let Some((id, action, text)) = refused {
            self.ask = Some((id, action, format!("{text}\n\nCut the power anyway?"), true));
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Network").size(18.0).color(theme::ink()));
            ui.add_space(12.0);
            let online = machines
                .iter()
                .filter(|m| matches!(m.status.as_str(), "online" | "busy"))
                .count();
            let mut bits = vec![
                format!("{} machines", machines.len()),
                format!("{online} online"),
            ];
            let watts: f64 = machines
                .iter()
                .filter_map(|m| m.plug["status"]["watts"].as_f64())
                .sum();
            if watts > 0.0 {
                bits.push(format!("{watts:.0} W on plugs"));
            }
            ui.label(
                RichText::new(bits.join("  ·  "))
                    .color(theme::muted())
                    .size(12.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("All").clicked() {
                    self.camera = Camera::default();
                    self.selected = None;
                    self.framed = 0;
                }
                ui.checkbox(&mut self.show_mesh, "Mesh");
            });
        });
        if !fleet {
            ui.label(
                RichText::new("No fleet coordinator behind this kimo: showing Tailscale only. `kimo serve --fleet <url>` adds power, plugs and the mesh.")
                    .color(theme::muted())
                    .size(12.0),
            );
        }
        ui.add_space(6.0);

        let area = ui.available_rect_before_wrap();
        let panel_width = 280f32.min(area.width() * 0.4);
        let stage = Rect::from_min_max(
            area.min,
            Pos2::new(area.right() - panel_width - 12.0, area.bottom()),
        );
        let panel = Rect::from_min_max(Pos2::new(stage.right() + 12.0, area.top()), area.max);
        ui.allocate_rect(area, Sense::hover());

        self.stage(ui, stage, &machines, &mesh, &models);
        let mut child = ui.new_child(UiBuilder::new().max_rect(panel));
        egui::ScrollArea::vertical()
            .id_salt("network-panel")
            .show(&mut child, |ui| {
                self.panel(
                    ui,
                    client,
                    &machines,
                    &mesh,
                    fleet,
                    credential,
                    note.as_deref(),
                    found.as_ref(),
                );
            });
    }

    fn stage(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        machines: &[Machine],
        mesh: &Value,
        models: &HashMap<String, Arc<Vec<Part>>>,
    ) {
        // Frame everything when the machines first arrive, and on "All".
        if self.framed != machines.len() && self.selected.is_none() {
            self.framed = machines.len();
            let points: Vec<[f32; 3]> = machines
                .iter()
                .flat_map(|m| {
                    let reach = 0.25;
                    [
                        [m.at[0] - reach, 0.0, m.at[1] - reach],
                        [m.at[0] + reach, 0.45, m.at[1] + reach],
                    ]
                })
                .collect();
            self.camera.fit(&points, rect);
        }
        let response = ui.interact(
            rect,
            ui.scope_id().with("network-stage"),
            Sense::click_and_drag(),
        );
        if response.dragged_by(egui::PointerButton::Primary) {
            let d = response.drag_delta();
            self.camera.yaw -= d.x * 0.006;
            self.camera.pitch = (self.camera.pitch + d.y * 0.006).clamp(0.08, 1.5);
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.camera.pan(response.drag_delta());
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.distance =
                    (self.camera.distance * (-scroll * 0.002).exp()).clamp(0.3, 40.0);
            }
        }

        // Labels above each machine, out of each other's way.
        let font = egui::FontId::proportional(11.5);
        let anchors: Vec<Option<Pos2>> = machines
            .iter()
            .map(|m| {
                self.camera
                    .project([m.at[0], top_of(&m.kind, models) + 0.05, m.at[1]], rect)
            })
            .collect();
        let sizes: Vec<Vec2> = machines
            .iter()
            .map(|m| {
                ui.fonts_mut(|f| {
                    f.layout_no_wrap(m.id.clone(), font.clone(), Color32::WHITE)
                        .size()
                }) + Vec2::new(22.0, 8.0)
            })
            .collect();
        let tags: Vec<(Pos2, Vec2)> = anchors
            .iter()
            .zip(&sizes)
            .map(|(a, s)| {
                (
                    a.unwrap_or(Pos2::new(-9999.0, -9999.0)) - Vec2::new(0.0, s.y),
                    *s,
                )
            })
            .collect();
        let lifts = declutter(&tags);
        let label_rects: Vec<Rect> = tags
            .iter()
            .zip(&lifts)
            .map(|((at, size), lift)| Rect::from_center_size(*at - Vec2::new(0.0, *lift), *size))
            .collect();

        if response.clicked() {
            if let Some(p) = response.interact_pointer_pos() {
                let by_label = label_rects.iter().position(|r| r.contains(p));
                let nearest = anchors
                    .iter()
                    .enumerate()
                    .filter_map(|(i, a)| a.map(|a| (i, a.distance(p))))
                    .filter(|(_, d)| *d < 40.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i);
                match by_label.or(nearest) {
                    Some(i) => {
                        let m = &machines[i];
                        self.selected = Some(m.id.clone());
                        self.camera.target = [m.at[0], top_of(&m.kind, models) * 0.5, m.at[1]];
                        let reach = stand_in(&m.kind).iter().fold(0.0f32, |a, b| a.max(*b));
                        self.camera.distance = (reach * 5.0).max(1.0);
                    }
                    None => self.selected = None,
                }
            }
        }

        // The 3D part.
        let status_color = |m: &Machine| match m.status.as_str() {
            "online" => theme::good(),
            "busy" => theme::accent(),
            "waking" => theme::warn(),
            "unreachable" => theme::bad(),
            _ => theme::muted(),
        };
        let at_of = |id: &str| machines.iter().find(|m| m.id == id);
        let mut lines: Vec<(Vec<[f32; 3]>, Color32)> = Vec::new();
        if self.show_mesh {
            for p in mesh_pairs(mesh) {
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|s| *s != p.a && *s != p.b)
                {
                    continue;
                }
                let (Some(a), Some(b)) = (at_of(&p.a), at_of(&p.b)) else {
                    continue;
                };
                let color = match p.state {
                    "good" => theme::good(),
                    "oneway" => theme::muted(),
                    "violation" => theme::bad(),
                    _ => theme::warn(),
                };
                let (ya, yb) = (
                    top_of(&a.kind, models) + 0.02,
                    top_of(&b.kind, models) + 0.02,
                );
                let length = ((a.at[0] - b.at[0]).powi(2) + (a.at[1] - b.at[1]).powi(2)).sqrt();
                let curve = |t: f32| {
                    let h = 4.0 * t * (1.0 - t) * (0.08 + length * 0.1);
                    [
                        a.at[0] + (b.at[0] - a.at[0]) * t,
                        ya + (yb - ya) * t + h,
                        a.at[1] + (b.at[1] - a.at[1]) * t,
                    ]
                };
                let mut segment = Vec::with_capacity(48);
                for i in 0..24 {
                    segment.push(curve(i as f32 / 24.0));
                    segment.push(curve((i + 1) as f32 / 24.0));
                }
                lines.push((segment, color));
            }
        }
        let new_models: Vec<(String, Arc<Vec<Part>>)> = models
            .iter()
            .filter(|(k, _)| !self.uploaded.contains(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (k, _) in &new_models {
            self.uploaded.insert(k.clone());
        }
        let frame = Frame {
            camera: self.camera,
            aspect: rect.width() / rect.height().max(1.0),
            machines: machines
                .iter()
                .map(|m| (m.kind.clone(), m.id.clone(), m.at, status_color(m)))
                .collect(),
            lines,
            grid: theme::line(),
            new_models,
            background: theme::background(),
        };
        let gl_scene = self.gl.clone();
        let callback = egui::PaintCallback {
            rect,
            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |info, painter| {
                paint(painter.gl(), &gl_scene, &frame, &info);
            })),
        };
        ui.painter().add(callback);
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, theme::line()),
            egui::StrokeKind::Inside,
        );

        let clip = ui.painter().with_clip_rect(rect);
        for (i, m) in machines.iter().enumerate() {
            let Some(anchor) = anchors[i] else { continue };
            let r = label_rects[i];
            if !rect.intersects(r) {
                continue;
            }
            let selected = self.selected.as_deref() == Some(m.id.as_str());
            if lifts[i] > 0.5 {
                clip.line_segment(
                    [Pos2::new(r.center().x, r.bottom()), anchor],
                    Stroke::new(1.0, theme::line()),
                );
            }
            clip.rect(
                r,
                2.0,
                theme::background(),
                Stroke::new(
                    1.0,
                    if selected {
                        theme::accent()
                    } else {
                        theme::line()
                    },
                ),
                egui::StrokeKind::Inside,
            );
            clip.circle_filled(
                Pos2::new(r.left() + 9.0, r.center().y),
                3.0,
                status_color(m),
            );
            clip.text(
                Pos2::new(r.left() + 16.0, r.center().y),
                egui::Align2::LEFT_CENTER,
                &m.id,
                font.clone(),
                theme::ink(),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn panel(
        &mut self,
        ui: &mut Ui,
        client: &Client,
        machines: &[Machine],
        mesh: &Value,
        fleet: bool,
        credential: bool,
        note: Option<&str>,
        found: Option<&Value>,
    ) {
        let ctx = ui.ctx().clone();
        if let Some(n) = note {
            ui.label(RichText::new(n).color(theme::muted()).size(12.0));
            ui.add_space(6.0);
        }
        if let Some((id, action, text, force)) = self.ask.clone() {
            ui.label(RichText::new(text).color(theme::warn()).size(12.0));
            ui.horizontal(|ui| {
                if ui.button("yes").clicked() {
                    self.act(client, &ctx, &id, &action, force);
                    self.ask = None;
                }
                if ui.button("no").clicked() {
                    self.ask = None;
                }
            });
            ui.add_space(8.0);
        }
        let row = |ui: &mut Ui, k: &str, v: &str| {
            if v.is_empty() {
                return;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(k).color(theme::muted()).size(11.5));
                ui.label(RichText::new(v).color(theme::text()).size(11.5));
            });
        };
        let chosen = self
            .selected
            .as_ref()
            .and_then(|s| machines.iter().find(|m| m.id == *s))
            .cloned();
        match chosen {
            None => {
                ui.label(
                    RichText::new(
                        "Click a machine. Drag to turn, right-drag to move, scroll to zoom.",
                    )
                    .color(theme::muted())
                    .size(12.0),
                );
                ui.add_space(8.0);
                theme::section(ui, "Machines");
                for m in machines {
                    ui.horizontal(|ui| {
                        theme::dot(
                            ui,
                            match m.status.as_str() {
                                "online" | "busy" => theme::good(),
                                "waking" => theme::warn(),
                                "unreachable" => theme::bad(),
                                _ => theme::muted(),
                            },
                        );
                        if ui
                            .link(RichText::new(&m.id).size(12.0).color(theme::ink()))
                            .clicked()
                        {
                            self.selected = Some(m.id.clone());
                        }
                        let w = m.plug["status"]["watts"].as_f64();
                        if let Some(w) = w {
                            ui.label(
                                RichText::new(format!("{w} W"))
                                    .size(11.0)
                                    .color(theme::muted()),
                            );
                        }
                    });
                }
                if fleet {
                    theme::section(ui, "Add a machine");
                    if ui.button("find machines").clicked() {
                        let (c, s, ctx) = (client.clone(), self.shared.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            let result = c.call_timeout("GET", "/fleet/discover", None, 60);
                            let mut g = s.lock().unwrap_or_else(|e| e.into_inner());
                            match result {
                                Ok(v) => g.found = Some(v),
                                Err(e) => g.note = Some(e.to_string()),
                            }
                            ctx.request_repaint();
                        });
                    }
                    for m in found
                        .and_then(|f| f["machines"].as_array())
                        .into_iter()
                        .flatten()
                    {
                        if m["in_fleet"] == true || m["self"] == true || m["can_run_kimo"] != true {
                            continue;
                        }
                        let (name, address) = (
                            m["name"].as_str().unwrap_or(""),
                            m["address"].as_str().unwrap_or(""),
                        );
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(format!("{name} {address}")).size(11.5));
                            if ui.small_button("add").clicked() {
                                let (c, s, ctx, body) = (client.clone(), self.shared.clone(), ctx.clone(), json!({ "address": address }));
                                self.last = None;
                                std::thread::spawn(move || {
                                    let r = c.call_timeout("POST", "/fleet/adopt", Some(&body), 30);
                                    let mut g = s.lock().unwrap_or_else(|e| e.into_inner());
                                    g.note = Some(match r {
                                        Ok(_) => "adding it: installing kimo and pairing, follow it in the Fleet tab".into(),
                                        Err(e) => e.to_string(),
                                    });
                                    ctx.request_repaint();
                                });
                            }
                        });
                    }
                }
                let problems = mesh_pairs(mesh)
                    .iter()
                    .filter(|p| p.state == "problem" || p.state == "violation")
                    .count();
                if mesh["links"].is_array() {
                    theme::section(ui, "Mesh");
                    let pairs = mesh_pairs(mesh).len();
                    ui.label(
                        RichText::new(format!("{pairs} pairs, {problems} need attention"))
                            .size(12.0)
                            .color(if problems > 0 {
                                theme::warn()
                            } else {
                                theme::muted()
                            }),
                    );
                    for v in mesh["violations"].as_array().into_iter().flatten() {
                        ui.label(
                            RichText::new(v.as_str().unwrap_or(""))
                                .size(11.5)
                                .color(theme::bad()),
                        );
                    }
                    if ui.button("check now").clicked() {
                        let (c, s, ctx) = (client.clone(), self.shared.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            let r = c.post("/fleet/mesh/check", json!({}));
                            s.lock().unwrap_or_else(|e| e.into_inner()).note = Some(r.map_or_else(
                                |e| e.to_string(),
                                |_| "Checking the mesh: lines update when it is done".to_string(),
                            ));
                            ctx.request_repaint();
                        });
                    }
                }
                if fleet && !credential && machines.iter().any(|m| m.plug.is_object()) {
                    theme::section(ui, "Smart plugs");
                    ui.label(
                        RichText::new(
                            "TP-Link account password (the coordinator keeps only a hash)",
                        )
                        .size(11.5)
                        .color(theme::muted()),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(true)
                            .desired_width(180.0),
                    );
                    if ui.button("save").clicked() && !self.password.is_empty() {
                        let body = json!({ "password": std::mem::take(&mut self.password) });
                        let (c, s, ctx) = (client.clone(), self.shared.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            let r = c.put("/fleet/plug-credential", body);
                            s.lock().unwrap_or_else(|e| e.into_inner()).note =
                                Some(r.map_or_else(|e| e.to_string(), |_| "Saved".to_string()));
                            ctx.request_repaint();
                        });
                    }
                }
            }
            Some(m) => {
                if !m.role.is_empty() {
                    ui.label(
                        RichText::new(m.role.to_uppercase())
                            .size(10.5)
                            .color(theme::accent()),
                    );
                }
                ui.label(RichText::new(&m.id).size(17.0).color(theme::ink()));
                ui.label(RichText::new(&m.status).size(12.0).color(theme::muted()));
                if !m.error.is_empty() {
                    ui.label(RichText::new(&m.error).size(11.0).color(theme::muted()));
                }
                if fleet {
                    theme::section(ui, "Power");
                    ui.horizontal_wrapped(|ui| {
                        if m.can_wake && matches!(m.status.as_str(), "off" | "unreachable") && ui.button("wake").clicked() {
                            self.act(client, &ctx, &m.id, "wake", false);
                        }
                        if m.plug.is_object() && credential {
                            let on = m.plug["status"]["on"].as_bool();
                            if on == Some(false) && ui.button("plug on").clicked() {
                                self.act(client, &ctx, &m.id, "on", false);
                            }
                            if on != Some(false) && ui.button("restart by plug").clicked() {
                                self.ask = Some((m.id.clone(), "cycle".into(), format!("Restart {} by cutting its power? Anything running there stops at once, like pulling the cable.", m.id), false));
                            }
                            let through_itself = m.plug["via"].as_str() == Some(m.id.as_str());
                            if on != Some(false) && !through_itself && ui.button("plug off").clicked() {
                                self.ask = Some((m.id.clone(), "off".into(), format!("Cut the power to {}? Anything running there stops at once, like pulling the cable.", m.id), false));
                            }
                        }
                    });
                    if m.plug.is_object() {
                        let through = m.plug["via"]
                            .as_str()
                            .map(|v| format!(" through {v}"))
                            .unwrap_or_default();
                        row(
                            ui,
                            "Plug",
                            &format!("{}{through}", m.plug["address"].as_str().unwrap_or("")),
                        );
                        row(ui, "Now", &plug_line(&m.plug));
                        if !credential {
                            ui.label(
                                RichText::new(
                                    "Save your TP-Link password below the machine list to use it.",
                                )
                                .size(11.0)
                                .color(theme::muted()),
                            );
                        }
                    }
                }
                theme::section(ui, "Network");
                row(ui, "ZeroTier", &m.zerotier);
                row(ui, "Tailscale", &m.tailscale);
                if !m.alias.is_empty() {
                    row(ui, "SSH", &format!("ssh {}", m.alias));
                }
                let pairs: Vec<Pair> = mesh_pairs(mesh)
                    .into_iter()
                    .filter(|p| p.a == m.id || p.b == m.id)
                    .collect();
                if !pairs.is_empty() {
                    let bad = pairs
                        .iter()
                        .filter(|p| p.state == "problem" || p.state == "violation")
                        .count();
                    row(
                        ui,
                        "Mesh",
                        &if bad > 0 {
                            format!("{bad} of {} links need attention", pairs.len())
                        } else {
                            format!("{} links as intended", pairs.len())
                        },
                    );
                }
                let f = &m.facts;
                if f.is_object() {
                    theme::section(ui, "Machine");
                    row(ui, "Model", f["model"].as_str().unwrap_or(""));
                    row(ui, "GPU", f["gpus"][0]["name"].as_str().unwrap_or(""));
                    if let Some(load) = f["load"][0].as_f64() {
                        row(ui, "Load", &format!("{load:.1} on {} CPUs", f["cpus"]));
                    }
                }
                if m.added {
                    ui.add_space(10.0);
                    if ui.button("remove from the fleet").clicked() {
                        self.ask = Some((m.id.clone(), "remove".into(), format!("Remove {} from the fleet? Its entry and its key go; kimo stays installed there.", m.id), false));
                    }
                }
            }
        }
    }
}

fn paint(
    gl: &glow::Context,
    scene: &Arc<Mutex<Option<GlScene>>>,
    frame: &Frame,
    info: &egui::PaintCallbackInfo,
) {
    let mut held = scene.lock().unwrap_or_else(|e| e.into_inner());
    if held.is_none() {
        *held = GlScene::new(gl);
    }
    let Some(s) = held.as_mut() else { return };
    for (kind, parts) in &frame.new_models {
        let list = parts
            .iter()
            .filter_map(|p| upload(gl, &p.positions, &p.normals, &p.indices, p.color))
            .collect();
        s.models.insert(kind.clone(), list);
    }
    let matrix = frame.camera.matrix(frame.aspect);
    unsafe {
        let v = info.viewport_in_pixels();
        gl.viewport(v.left_px, v.from_bottom_px, v.width_px, v.height_px);
        // Only this view's rectangle: the clear below must not reach what is around it.
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(v.left_px, v.from_bottom_px, v.width_px, v.height_px);
        let bg = rgb(frame.background);
        gl.clear_color(bg[0], bg[1], bg[2], 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        gl.enable(glow::DEPTH_TEST);
        gl.depth_func(glow::LEQUAL);
        gl.depth_mask(true);
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.use_program(Some(s.program));
        gl.uniform_matrix_4_f32_slice(s.mvp.as_ref(), false, &matrix);
        gl.uniform_1_f32(s.lit.as_ref(), 0.0);
    }
    // The floor's grid, every half metre.
    let mut grid = Vec::new();
    let mut v = -8.0f32;
    while v <= 10.0 {
        grid.push([v, 0.0, -6.0]);
        grid.push([v, 0.0, 6.0]);
        v += 0.5;
    }
    let mut w = -6.0f32;
    while w <= 6.0 {
        grid.push([-8.0, 0.0, w]);
        grid.push([10.0, 0.0, w]);
        w += 0.5;
    }
    s.draw_lines(gl, &grid, rgb(frame.grid));
    for (points, color) in &frame.lines {
        s.draw_lines(gl, points, rgb(*color));
    }
    for (kind, _, at, status) in &frame.machines {
        let top;
        if let Some(parts) = s.models.get(kind) {
            let m = scale_at(at[0], at[1], [1.0; 3], 0.0);
            let mvp = mul(&matrix, &m);
            for part in parts {
                s.draw_part(gl, part, &mvp, part.color, 1.0);
            }
            top = frame
                .new_models
                .iter()
                .find(|(k, _)| k == kind)
                .map(|(_, p)| {
                    p.iter()
                        .flat_map(|x| x.positions.iter().map(|v| v[1]))
                        .fold(0.0, f32::max)
                })
                .unwrap_or_else(|| stand_in(kind)[1]);
        } else {
            let size = stand_in(kind);
            let mvp = mul(&matrix, &scale_at(at[0], at[1], size, 0.0));
            s.draw_part(gl, &s.cube, &mvp, s.cube.color, 1.0);
            top = size[1];
        }
        let dot = 0.008;
        let mvp = mul(&matrix, &scale_at(at[0], at[1], [dot; 3], top + 0.002));
        s.draw_part(gl, &s.cube, &mvp, rgb(*status), 0.0);
    }
    unsafe {
        gl.bind_vertex_array(None);
        gl.use_program(None);
        gl.disable(glow::DEPTH_TEST);
        gl.enable(glow::BLEND);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_the_fleet_then_names() {
        assert_eq!(kind_of("nuc-11-extreme", "x", ""), "nucExtreme");
        assert_eq!(kind_of("rpi-zero-2w", "x", ""), "zero2");
        assert_eq!(kind_of("gaming-laptop", "x", ""), "laptop");
        assert_eq!(kind_of("", "jetson-dev", "linux"), "jetson");
        assert_eq!(kind_of("", "rpi5-ai", "linux"), "rpi5");
        assert_eq!(kind_of("", "iPhone", "iOS"), "phone");
        assert_eq!(kind_of("", "macbook-air", "macOS"), "macbookAir");
    }

    #[test]
    fn the_tailnet_and_the_fleet_are_one_list() {
        let tailnet = json!({ "devices": [
            { "name": "lab-ts", "host_name": "lab", "ips": ["100.64.0.5"], "os": "linux", "online": true, "self": false, "mine": true },
            { "name": "air", "host_name": "air", "ips": ["100.64.0.1"], "os": "macOS", "online": true, "self": true, "mine": true },
            { "name": "stranger", "ips": [], "os": "linux", "online": true, "mine": false },
        ]});
        let fleet = json!([
            { "id": "lab-ts", "state": "ready", "can_wake": true, "labels": [], "plug": { "address": "192.168.1.50" },
              "registry": { "tailscale": "lab-ts", "zerotier": "10.147.17.21", "user": "demo", "alias": "zlab", "model": "nuc-11-extreme", "role": "gpu-worker" } },
            { "id": "guest", "state": "ready", "labels": ["site:client"],
              "registry": { "zerotier": "10.147.17.60", "user": "demo", "model": "gaming-laptop" } },
            { "id": "gone", "state": "off", "labels": [], "registry": { "zerotier": "10.147.17.90" } },
        ]);
        let list = merge(&tailnet, &fleet, &[]);
        let m = |id: &str| list.iter().find(|m| m.id == id).unwrap();
        assert_eq!(list.len(), 4, "not the stranger");
        assert_eq!(m("lab-ts").tailscale, "100.64.0.5");
        assert_eq!(m("lab-ts").zerotier, "10.147.17.21");
        assert_eq!(m("lab-ts").alias, "zlab");
        assert_eq!(m("lab-ts").kind, "nucExtreme");
        assert_eq!(m("lab-ts").role, "gpu worker");
        assert!(m("lab-ts").can_wake);
        assert_eq!(m("guest").status, "online");
        assert_eq!(m("guest").zone, "site:client");
        assert_eq!(m("guest").alias, "zguest");
        assert_eq!(m("gone").status, "off");
        assert_eq!(m("air").zone, "core");
        // Nobody stands on top of anybody.
        for a in &list {
            for b in &list {
                if a.id != b.id {
                    assert!(
                        (a.at[0] - b.at[0]).abs() + (a.at[1] - b.at[1]).abs() > 0.3,
                        "{} {}",
                        a.id,
                        b.id
                    );
                }
            }
        }
    }

    #[test]
    fn mesh_pairs_follow_the_rule() {
        let link = |from: &str, to: &str, found: &str, expect: &str, intended: bool| json!({ "from": from, "to": to, "found": found, "expect": expect, "as_intended": intended, "network": "zerotier" });
        let report = json!({ "links": [
            link("a", "b", "ok", "key", true), link("b", "a", "ok", "key", true),
            link("a", "b", "check", "key", false),
            link("a", "k", "ok", "key", true), link("k", "a", "nokey", "password", true),
            link("b", "k", "ok", "key", true), link("k", "b", "ok", "password", false),
            link("a", "x", "down", "key", false),
        ]});
        let state = |a: &str, b: &str| {
            mesh_pairs(&report)
                .into_iter()
                .find(|p| p.a == a && p.b == b)
                .unwrap()
                .state
        };
        assert_eq!(state("a", "b"), "good");
        assert_eq!(state("a", "k"), "oneway");
        assert_eq!(state("b", "k"), "violation");
        assert_eq!(state("a", "x"), "problem");
        assert!(mesh_pairs(&Value::Null).is_empty());
    }

    #[test]
    fn plug_lines() {
        assert_eq!(plug_line(&Value::Null), "");
        assert_eq!(plug_line(&json!({ "address": "1.2.3.4" })), "not read yet");
        assert_eq!(plug_line(&json!({ "error": "no answer" })), "no answer");
        assert_eq!(
            plug_line(&json!({ "status": { "on": true, "watts": 41.5, "today_wh": 120 } })),
            "on, 41.5 W, 120 Wh today"
        );
        assert_eq!(plug_line(&json!({ "status": { "on": false } })), "off");
    }

    #[test]
    fn the_camera_projects_what_it_looks_at_to_the_middle() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(800.0, 400.0));
        let c = Camera::default();
        let p = c.project(c.target, rect).unwrap();
        assert!(
            (p.x - 500.0).abs() < 0.5 && (p.y - 250.0).abs() < 0.5,
            "{p:?}"
        );
        // Up on the floor is up on the screen.
        let above = c.project([c.target[0], 1.0, c.target[2]], rect).unwrap();
        assert!(above.y < p.y);
        // Behind the camera is nowhere.
        let eye = c.eye();
        let behind = [
            eye[0] * 2.0 - c.target[0],
            eye[1] * 2.0,
            eye[2] * 2.0 - c.target[2],
        ];
        assert_eq!(c.project(behind, rect), None);
    }

    #[test]
    fn framing_shows_everything() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let points = [[-4.0, 0.0, -2.0], [5.0, 0.5, 3.0], [1.0, 0.2, 0.0]];
        let mut c = Camera::default();
        c.fit(&points, rect);
        for p in points {
            assert!(rect.contains(c.project(p, rect).unwrap()), "{p:?}");
        }
        assert!(c.distance < 60.0);
    }

    #[test]
    fn labels_do_not_cover_each_other() {
        let size = Vec2::new(80.0, 20.0);
        let at = Pos2::new(100.0, 100.0);
        let lifts = declutter(&[(at, size), (at, size), (at + Vec2::new(300.0, 0.0), size)]);
        assert_eq!(lifts[2], 0.0, "far away: stays");
        assert!((lifts[0] - lifts[1]).abs() >= 20.0, "{lifts:?}");
    }
}
