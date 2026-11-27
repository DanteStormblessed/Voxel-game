//! Grafos de nodos del terreno: cada nodo da un número por columna del mundo (x, z).
//!
//! Un grafo se escribe como datos (una tabla de nodos con nombre; ver `content/README.md`)
//! y se compila a un programa plano: una lista de operaciones en orden, cada una con su
//! registro. Evaluarlo no reserva memoria ni busca nombres. Este módulo no sabe de biomas
//! ni de bloques: solo de números.

use std::collections::BTreeMap;

use anyhow::{bail, Context};
use serde::Deserialize;

use crate::noise::{fbm2, hash2, perlin2, smoothstep};

/// Entradas que existen en cualquier grafo, en este orden de registro: posición en
/// metros y posición dentro de la zona jugable (0-1, de oeste a este y de norte a sur).
pub const BUILTINS: [&str; 4] = ["x", "z", "u", "v"];
/// Tope de operaciones de un programa: sus registros caben en la pila.
pub const MAX_OPS: usize = 160;

/// Entrada de un nodo: un número o el nombre de otro nodo.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Input {
    Value(f32),
    Node(String),
}

/// Tamaño de los rasgos de un ruido, en metros: igual en los dos ejes o `[x, z]`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Scale {
    Uniform(f32),
    Axes([f32; 2]),
}

impl Scale {
    fn axes(self) -> [f32; 2] {
        match self {
            Scale::Uniform(s) => [s, s],
            Scale::Axes(axes) => axes,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CellMode {
    /// Distancia en metros al borde de la celda (0 en la grieta entre dos celdas).
    #[default]
    Edge,
    /// Distancia en metros al centro de la celda.
    Center,
    /// Un valor fijo al azar (0-1) por celda.
    Value,
}

fn default_octaves() -> u32 {
    4
}
fn default_jitter() -> f32 {
    0.8
}
fn one() -> f32 {
    1.0
}

/// Un nodo tal como se escribe en el archivo.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeDef {
    Constant {
        value: f32,
    },
    /// Ruido fractal suave, en [-1, 1].
    Noise {
        scale: Scale,
        #[serde(default = "default_octaves")]
        octaves: u32,
        seed: Option<u32>,
        /// Deformación del dominio, en metros: rompe las formas demasiado regulares.
        #[serde(default)]
        warp: f32,
        warp_scale: Option<f32>,
    },
    /// Ruido de crestas, en [0, 1]: 1 en la línea de cumbres.
    Ridge {
        scale: Scale,
        #[serde(default = "default_octaves")]
        octaves: u32,
        seed: Option<u32>,
        #[serde(default)]
        warp: f32,
        warp_scale: Option<f32>,
    },
    /// Celdas irregulares (Voronoi): mesetas, grietas, placas.
    Cells {
        scale: f32,
        #[serde(default)]
        mode: CellMode,
        #[serde(default = "default_jitter")]
        jitter: f32,
        seed: Option<u32>,
        #[serde(default)]
        warp: f32,
        warp_scale: Option<f32>,
    },
    /// Manchas redondas repartidas al azar: 1 en el centro, 0 a partir del radio.
    Spots {
        spacing: f32,
        radius: f32,
        #[serde(default = "one")]
        chance: f32,
        /// Cuánto puede encoger el radio de una mancha a otra (0-1).
        #[serde(default)]
        vary: f32,
        seed: Option<u32>,
    },
    /// Una mancha redonda en un punto fijo del mundo.
    Point {
        x: f32,
        z: f32,
        radius: f32,
    },
    Add {
        inputs: Vec<Input>,
    },
    Mul {
        inputs: Vec<Input>,
    },
    Min {
        inputs: Vec<Input>,
    },
    Max {
        inputs: Vec<Input>,
    },
    Sub {
        a: Input,
        b: Input,
    },
    /// Mezcla `a` y `b`: 0 = a, 1 = b.
    Mix {
        a: Input,
        b: Input,
        t: Input,
    },
    Clamp {
        input: Input,
        min: f32,
        max: f32,
    },
    Abs {
        input: Input,
    },
    Pow {
        input: Input,
        exponent: f32,
    },
    /// 0 por debajo de `from`, 1 por encima de `to`, suave entre medias. Con `from` >
    /// `to` va al revés.
    Smoothstep {
        input: Input,
        from: f32,
        to: f32,
    },
    /// Curva por puntos `[entrada, salida]`: la forma más directa de dibujar un perfil.
    Curve {
        input: Input,
        points: Vec<[f32; 2]>,
        #[serde(default)]
        smooth: bool,
    },
    /// Escalona la entrada en terrazas de `step` de alto. `sharpness`: 0 = sin efecto,
    /// 1 = escalones verticales.
    Terrace {
        input: Input,
        step: f32,
        #[serde(default = "one")]
        sharpness: f32,
    },
}

/// Qué clase de valor lleva un parámetro de un nodo. Es lo que necesita un editor para
/// dibujar sus puertos y sus campos sin conocer cada tipo de nodo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    /// Un número o el nombre de otro nodo: un puerto de entrada.
    Input,
    /// Una lista de entradas (números o nodos): un puerto por elemento.
    Inputs,
    Number,
    /// Número entero.
    Integer,
    /// Número entero opcional que fija el dibujo de un ruido.
    Seed,
    /// Tamaño en metros: un número o `[x, z]`.
    Scale,
    /// Uno de unos nombres fijos.
    Choice(&'static [&'static str]),
    Bool,
    /// Puntos `[entrada, salida]` de una curva.
    Points,
}

/// Un parámetro de un tipo de nodo.
#[derive(Clone, Copy, Debug)]
pub struct ParamSchema {
    pub name: &'static str,
    pub kind: ParamKind,
    /// Si falta, el nodo no es válido. Los opcionales tienen un valor por defecto.
    pub required: bool,
    /// Valor con el que nace (o el que vale si falta, en los opcionales).
    pub default: f32,
    pub help: &'static str,
}

/// Un tipo de nodo: nombre en el archivo, grupo y parámetros.
#[derive(Clone, Copy, Debug)]
pub struct NodeSchema {
    pub kind: &'static str,
    /// Grupo en el que se ofrece en un editor.
    pub group: &'static str,
    pub help: &'static str,
    pub params: &'static [ParamSchema],
}

const fn param(name: &'static str, kind: ParamKind, default: f32, help: &'static str) -> ParamSchema {
    ParamSchema { name, kind, required: true, default, help }
}

const fn optional(name: &'static str, kind: ParamKind, default: f32, help: &'static str) -> ParamSchema {
    ParamSchema { name, kind, required: false, default, help }
}

const NOISE_PARAMS: &[ParamSchema] = &[
    param("scale", ParamKind::Scale, 200.0, "Size of the features in meters (or [x, z])"),
    optional("octaves", ParamKind::Integer, 4.0, "Layers of detail, 1 to 8"),
    optional("seed", ParamKind::Seed, 0.0, "Same seed = same pattern as other nodes"),
    optional("warp", ParamKind::Number, 0.0, "Distortion in meters: breaks regular shapes"),
    optional("warp_scale", ParamKind::Number, 0.0, "Size of the distortion (default: twice the scale)"),
];

/// Todos los tipos de nodo. Debe coincidir con `NodeDef` (lo comprueba un test).
pub const NODE_TYPES: &[NodeSchema] = &[
    NodeSchema {
        kind: "constant",
        group: "Values",
        help: "A fixed number",
        params: &[param("value", ParamKind::Number, 0.0, "The number")],
    },
    NodeSchema { kind: "noise", group: "Shapes", help: "Smooth noise, -1 to 1", params: NOISE_PARAMS },
    NodeSchema {
        kind: "ridge",
        group: "Shapes",
        help: "Ridges: 1 on the crest line, 0 in the valleys",
        params: NOISE_PARAMS,
    },
    NodeSchema {
        kind: "cells",
        group: "Shapes",
        help: "Irregular cells: plateaus, cracks, plates",
        params: &[
            param("scale", ParamKind::Number, 150.0, "Size of a cell in meters"),
            optional(
                "mode",
                ParamKind::Choice(&["edge", "center", "value"]),
                0.0,
                "Meters to the edge, meters to the center, or a value per cell",
            ),
            optional("jitter", ParamKind::Number, 0.8, "How irregular the cells are, 0 to 1"),
            optional("seed", ParamKind::Seed, 0.0, "Same seed = same cells as other nodes"),
            optional("warp", ParamKind::Number, 0.0, "Distortion in meters"),
            optional("warp_scale", ParamKind::Number, 0.0, "Size of the distortion"),
        ],
    },
    NodeSchema {
        kind: "spots",
        group: "Shapes",
        help: "Round spots scattered at random: 1 in the middle, 0 from the radius",
        params: &[
            param("spacing", ParamKind::Number, 800.0, "Distance between spots in meters"),
            param("radius", ParamKind::Number, 250.0, "Radius of a spot in meters"),
            optional("chance", ParamKind::Number, 1.0, "Fraction of the places that have a spot"),
            optional("vary", ParamKind::Number, 0.0, "How much a radius can shrink, 0 to 1"),
            optional("seed", ParamKind::Seed, 0.0, "Same seed = same spots as other nodes"),
        ],
    },
    NodeSchema {
        kind: "point",
        group: "Shapes",
        help: "A round spot at a fixed place of the world",
        params: &[
            param("x", ParamKind::Number, 0.0, "East-west position in meters"),
            param("z", ParamKind::Number, 0.0, "North-south position in meters"),
            param("radius", ParamKind::Number, 500.0, "Radius in meters"),
        ],
    },
    NodeSchema {
        kind: "add",
        group: "Math",
        help: "Sum of the inputs",
        params: &[param("inputs", ParamKind::Inputs, 0.0, "Numbers or nodes")],
    },
    NodeSchema {
        kind: "mul",
        group: "Math",
        help: "Product of the inputs; stops at the first 0 (put masks first)",
        params: &[param("inputs", ParamKind::Inputs, 1.0, "Numbers or nodes")],
    },
    NodeSchema {
        kind: "min",
        group: "Math",
        help: "Smallest of the inputs",
        params: &[param("inputs", ParamKind::Inputs, 0.0, "Numbers or nodes")],
    },
    NodeSchema {
        kind: "max",
        group: "Math",
        help: "Largest of the inputs",
        params: &[param("inputs", ParamKind::Inputs, 0.0, "Numbers or nodes")],
    },
    NodeSchema {
        kind: "sub",
        group: "Math",
        help: "a - b",
        params: &[param("a", ParamKind::Input, 0.0, "First value"), param("b", ParamKind::Input, 0.0, "Subtracted")],
    },
    NodeSchema {
        kind: "mix",
        group: "Math",
        help: "a when t is 0, b when t is 1, a blend in between",
        params: &[
            param("a", ParamKind::Input, 0.0, "Value at t = 0"),
            param("b", ParamKind::Input, 1.0, "Value at t = 1"),
            param("t", ParamKind::Input, 0.5, "Blend, 0 to 1"),
        ],
    },
    NodeSchema {
        kind: "clamp",
        group: "Math",
        help: "Keeps the input between min and max",
        params: &[
            param("input", ParamKind::Input, 0.0, "Value"),
            param("min", ParamKind::Number, 0.0, "Lowest"),
            param("max", ParamKind::Number, 1.0, "Highest"),
        ],
    },
    NodeSchema {
        kind: "abs",
        group: "Math",
        help: "Absolute value",
        params: &[param("input", ParamKind::Input, 0.0, "Value")],
    },
    NodeSchema {
        kind: "pow",
        group: "Math",
        help: "Power (negative inputs count as 0)",
        params: &[param("input", ParamKind::Input, 0.0, "Value"), param("exponent", ParamKind::Number, 2.0, "Power")],
    },
    NodeSchema {
        kind: "smoothstep",
        group: "Curves",
        help: "0 before 'from', 1 after 'to', smooth in between (reversed if from > to)",
        params: &[
            param("input", ParamKind::Input, 0.0, "Value"),
            param("from", ParamKind::Number, 0.0, "Where it starts rising"),
            param("to", ParamKind::Number, 1.0, "Where it reaches 1"),
        ],
    },
    NodeSchema {
        kind: "curve",
        group: "Curves",
        help: "A curve drawn by points [input, output]",
        params: &[
            param("input", ParamKind::Input, 0.0, "Value"),
            param("points", ParamKind::Points, 0.0, "Points [input, output]"),
            optional("smooth", ParamKind::Bool, 0.0, "Smooth between points"),
        ],
    },
    NodeSchema {
        kind: "terrace",
        group: "Curves",
        help: "Steps the input into terraces",
        params: &[
            param("input", ParamKind::Input, 0.0, "Value"),
            param("step", ParamKind::Number, 10.0, "Height of a step"),
            optional("sharpness", ParamKind::Number, 1.0, "0 = no effect, 1 = vertical steps"),
        ],
    },
];

/// El esquema de un tipo de nodo.
pub fn schema(kind: &str) -> Option<&'static NodeSchema> {
    NODE_TYPES.iter().find(|schema| schema.kind == kind)
}

impl NodeSchema {
    pub fn param(&self, name: &str) -> Option<&'static ParamSchema> {
        self.params.iter().find(|param| param.name == name)
    }

    /// Un nodo nuevo de este tipo, con sus parámetros obligatorios a su valor inicial.
    pub fn new_node(&self) -> toml::Table {
        let mut table = toml::Table::new();
        table.insert("type".into(), toml::Value::String(self.kind.into()));
        for param in self.params.iter().filter(|param| param.required) {
            let number = toml::Value::Float(param.default as f64);
            let value = match param.kind {
                ParamKind::Inputs => toml::Value::Array(vec![number.clone(), number]),
                ParamKind::Points => toml::Value::Array(vec![
                    toml::Value::Array(vec![toml::Value::Float(0.0), toml::Value::Float(0.0)]),
                    toml::Value::Array(vec![toml::Value::Float(1.0), toml::Value::Float(1.0)]),
                ]),
                ParamKind::Choice(options) => toml::Value::String(options[0].into()),
                ParamKind::Bool => toml::Value::Boolean(param.default != 0.0),
                _ => number,
            };
            table.insert(param.name.into(), value);
        }
        table
    }
}

type Reg = u16;

#[derive(Clone, Debug)]
enum Op {
    Builtin,
    Const(f32),
    Noise { inv: [f64; 2], octaves: u32, seed: u32, warp: f32, warp_inv: f64, ridge: bool },
    Cells { inv: f64, scale: f32, mode: CellMode, jitter: f32, seed: u32, warp: f32, warp_inv: f64 },
    Spots { spacing: f64, radius: f32, chance: f32, vary: f32, seed: u32 },
    Point { x: f64, z: f64, radius: f32 },
    Add(Box<[Reg]>),
    Mul(Box<[Reg]>),
    Min(Box<[Reg]>),
    Max(Box<[Reg]>),
    Sub(Reg, Reg),
    Mix(Reg, Reg, Reg),
    Clamp(Reg, f32, f32),
    Abs(Reg),
    Pow(Reg, f32),
    Smoothstep(Reg, f32, f32),
    Curve(Reg, Box<[[f32; 2]]>, bool),
    Terrace(Reg, f32, f32),
}

/// Punto del mundo en el que se evalúa un programa.
#[derive(Clone, Copy, Debug)]
pub struct At {
    pub x: f64,
    pub z: f64,
    pub u: f32,
    pub v: f32,
}

/// Registros de una evaluación: el valor de cada nodo.
pub type Regs = [f32; MAX_OPS];

/// Un grafo compilado: solo los nodos que hacen falta para las salidas pedidas.
#[derive(Clone, Debug, Default)]
pub struct Program {
    ops: Vec<Op>,
    /// Registros que hay que calcular en cada evaluación.
    roots: Vec<Reg>,
}

fn fnv(text: &str) -> u32 {
    text.bytes().fold(0x811C_9DC5u32, |hash, byte| (hash ^ byte as u32).wrapping_mul(0x0100_0193))
}

struct Compiler<'a> {
    nodes: &'a BTreeMap<String, NodeDef>,
    seed: u32,
    ops: Vec<Op>,
    /// `None` mientras el nodo se está compilando: volver a él es un ciclo.
    done: BTreeMap<&'a str, Option<Reg>>,
}

impl<'a> Compiler<'a> {
    fn push(&mut self, op: Op) -> anyhow::Result<Reg> {
        if self.ops.len() >= MAX_OPS {
            bail!("the graph is too large (more than {MAX_OPS} operations)");
        }
        self.ops.push(op);
        Ok((self.ops.len() - 1) as Reg)
    }

    fn input(&mut self, input: &'a Input) -> anyhow::Result<Reg> {
        match input {
            Input::Value(value) => self.push(Op::Const(*value)),
            Input::Node(name) => self.node(name),
        }
    }

    fn inputs(&mut self, inputs: &'a [Input]) -> anyhow::Result<Box<[Reg]>> {
        if inputs.is_empty() {
            bail!("\"inputs\" is empty");
        }
        inputs.iter().map(|input| self.input(input)).collect()
    }

    fn node(&mut self, name: &'a str) -> anyhow::Result<Reg> {
        if let Some(index) = BUILTINS.iter().position(|builtin| *builtin == name) {
            return Ok(index as Reg);
        }
        match self.done.get(name) {
            Some(Some(reg)) => return Ok(*reg),
            Some(None) => bail!("node \"{name}\" depends on itself"),
            None => {}
        }
        let Some((key, def)) = self.nodes.get_key_value(name) else {
            bail!("unknown node \"{name}\"");
        };
        self.done.insert(key.as_str(), None);
        let reg = self.build(name, def).with_context(|| format!("in node \"{name}\""))?;
        self.done.insert(key.as_str(), Some(reg));
        Ok(reg)
    }

    /// Semilla de un nodo: por su nombre, o por su `seed` si quiere compartir ruido.
    fn node_seed(&self, name: &str, seed: Option<u32>) -> u32 {
        let own = seed.map_or_else(|| fnv(name), |seed| seed.wrapping_mul(0x9E37_79B9) ^ 0x5EED);
        hash2(own as i32, 0x51, self.seed)
    }

    fn build(&mut self, name: &str, def: &'a NodeDef) -> anyhow::Result<Reg> {
        let positive = |value: f32, what: &str| {
            if value > 0.0 && value.is_finite() {
                Ok(value)
            } else {
                Err(anyhow::anyhow!("\"{what}\" must be greater than 0"))
            }
        };
        let op = match def {
            NodeDef::Constant { value } => Op::Const(*value),
            NodeDef::Noise { scale, octaves, seed, warp, warp_scale }
            | NodeDef::Ridge { scale, octaves, seed, warp, warp_scale } => {
                let [sx, sz] = scale.axes();
                positive(sx, "scale")?;
                positive(sz, "scale")?;
                if !(1..=8).contains(octaves) {
                    bail!("\"octaves\" must be between 1 and 8");
                }
                let warp_scale = positive(warp_scale.unwrap_or(sx.max(sz) * 2.0), "warp_scale")?;
                Op::Noise {
                    inv: [1.0 / sx as f64, 1.0 / sz as f64],
                    octaves: *octaves,
                    seed: self.node_seed(name, *seed),
                    warp: *warp,
                    warp_inv: 1.0 / warp_scale as f64,
                    ridge: matches!(def, NodeDef::Ridge { .. }),
                }
            }
            NodeDef::Cells { scale, mode, jitter, seed, warp, warp_scale } => {
                positive(*scale, "scale")?;
                let warp_scale = positive(warp_scale.unwrap_or(scale * 2.0), "warp_scale")?;
                Op::Cells {
                    inv: 1.0 / *scale as f64,
                    scale: *scale,
                    mode: *mode,
                    jitter: jitter.clamp(0.0, 1.0),
                    seed: self.node_seed(name, *seed),
                    warp: *warp,
                    warp_inv: 1.0 / warp_scale as f64,
                }
            }
            NodeDef::Spots { spacing, radius, chance, vary, seed } => {
                positive(*spacing, "spacing")?;
                positive(*radius, "radius")?;
                if radius > spacing {
                    bail!("\"radius\" cannot be greater than \"spacing\"");
                }
                Op::Spots {
                    spacing: *spacing as f64,
                    radius: *radius,
                    chance: chance.clamp(0.0, 1.0),
                    vary: vary.clamp(0.0, 1.0),
                    seed: self.node_seed(name, *seed),
                }
            }
            NodeDef::Point { x, z, radius } => {
                Op::Point { x: *x as f64, z: *z as f64, radius: positive(*radius, "radius")? }
            }
            NodeDef::Add { inputs } => Op::Add(self.inputs(inputs)?),
            NodeDef::Mul { inputs } => Op::Mul(self.inputs(inputs)?),
            NodeDef::Min { inputs } => Op::Min(self.inputs(inputs)?),
            NodeDef::Max { inputs } => Op::Max(self.inputs(inputs)?),
            NodeDef::Sub { a, b } => Op::Sub(self.input(a)?, self.input(b)?),
            NodeDef::Mix { a, b, t } => Op::Mix(self.input(a)?, self.input(b)?, self.input(t)?),
            NodeDef::Clamp { input, min, max } => Op::Clamp(self.input(input)?, *min, *max),
            NodeDef::Abs { input } => Op::Abs(self.input(input)?),
            NodeDef::Pow { input, exponent } => Op::Pow(self.input(input)?, *exponent),
            NodeDef::Smoothstep { input, from, to } => {
                if from == to {
                    bail!("\"from\" and \"to\" cannot be equal");
                }
                Op::Smoothstep(self.input(input)?, *from, *to)
            }
            NodeDef::Curve { input, points, smooth } => {
                if points.is_empty() {
                    bail!("\"points\" is empty");
                }
                let mut points = points.clone();
                points.sort_by(|a, b| a[0].total_cmp(&b[0]));
                Op::Curve(self.input(input)?, points.into(), *smooth)
            }
            NodeDef::Terrace { input, step, sharpness } => {
                Op::Terrace(self.input(input)?, positive(*step, "step")?, sharpness.clamp(0.0, 1.0))
            }
        };
        self.push(op)
    }
}

impl Program {
    /// Compila los nodos necesarios para `roots`. Devuelve el programa y el registro de
    /// cada raíz, en el mismo orden.
    pub fn compile(
        nodes: &BTreeMap<String, NodeDef>,
        roots: &[&str],
        seed: u32,
    ) -> anyhow::Result<(Program, Vec<Reg>)> {
        for builtin in BUILTINS {
            if nodes.contains_key(builtin) {
                bail!("\"{builtin}\" is a built-in input and cannot be a node name");
            }
        }
        let mut compiler = Compiler {
            nodes,
            seed,
            ops: vec![Op::Builtin; BUILTINS.len()],
            done: BTreeMap::new(),
        };
        let mut regs = Vec::with_capacity(roots.len());
        for root in roots {
            // Un nombre que no está en el mapa no puede guardarse como clave prestada:
            // `node` solo guarda claves del propio mapa, así que basta con llamarlo.
            let reg = match nodes.get_key_value(*root) {
                Some((key, _)) => compiler.node(key.as_str())?,
                None => match BUILTINS.iter().position(|builtin| builtin == root) {
                    Some(index) => index as Reg,
                    None => bail!("unknown node \"{root}\""),
                },
            };
            regs.push(reg);
        }
        Ok((Program { ops: compiler.ops, roots: regs.clone() }, regs))
    }

    /// Un programa sin nodos propios no necesita evaluarse.
    pub fn is_trivial(&self) -> bool {
        self.ops.len() <= BUILTINS.len()
    }

    /// Calcula las raíces del programa (y solo lo que hace falta para ellas) y deja sus
    /// valores en `regs`.
    ///
    /// La evaluación es perezosa: un `mix` con `t` en 0 o en 1 no calcula la rama que no
    /// usa, y un `mul` se detiene en el primer factor que vale 0. Por eso conviene poner
    /// las máscaras (la costa, la cordillera) delante: en mar abierto no se calcula la
    /// tierra, ni las cumbres en el valle.
    pub fn eval(&self, at: At, regs: &mut Regs) {
        regs[0] = at.x as f32;
        regs[1] = at.z as f32;
        regs[2] = at.u;
        regs[3] = at.v;
        let mut done = [0u64; MAX_OPS.div_ceil(64)];
        done[0] = (1 << BUILTINS.len()) - 1;
        for &root in &self.roots {
            self.get(root, at, regs, &mut done);
        }
    }

    fn get(&self, reg: Reg, at: At, regs: &mut Regs, done: &mut [u64; MAX_OPS.div_ceil(64)]) -> f32 {
        let index = reg as usize;
        if done[index / 64] >> (index % 64) & 1 == 1 {
            return regs[index];
        }
        let value = match &self.ops[index] {
            Op::Builtin => regs[index],
            Op::Const(value) => *value,
            Op::Noise { inv, octaves, seed, warp, warp_inv, ridge } => {
                let (x, z) = warped(at, *warp, *warp_inv, *seed);
                let value = fbm2(x * inv[0], z * inv[1], *octaves, *seed);
                if *ridge {
                    (1.0 - value.abs() * 2.0).clamp(0.0, 1.0)
                } else {
                    value
                }
            }
            Op::Cells { inv, scale, mode, jitter, seed, warp, warp_inv } => {
                let (x, z) = warped(at, *warp, *warp_inv, *seed);
                cells(x * inv, z * inv, *mode, *jitter, *seed) * if *mode == CellMode::Value { 1.0 } else { *scale }
            }
            Op::Spots { spacing, radius, chance, vary, seed } => {
                spots(at.x, at.z, *spacing, *radius, *chance, *vary, *seed)
            }
            Op::Point { x, z, radius } => {
                let distance = ((at.x - x).powi(2) + (at.z - z).powi(2)).sqrt() as f32;
                (1.0 - distance / radius).max(0.0)
            }
            Op::Add(inputs) => {
                let mut sum = 0.0;
                for &input in inputs.iter() {
                    sum += self.get(input, at, regs, done);
                }
                sum
            }
            Op::Mul(inputs) => {
                let mut product = 1.0;
                for &input in inputs.iter() {
                    product *= self.get(input, at, regs, done);
                    if product == 0.0 {
                        break;
                    }
                }
                product
            }
            Op::Min(inputs) => {
                let mut min = f32::INFINITY;
                for &input in inputs.iter() {
                    min = min.min(self.get(input, at, regs, done));
                }
                min
            }
            Op::Max(inputs) => {
                let mut max = f32::NEG_INFINITY;
                for &input in inputs.iter() {
                    max = max.max(self.get(input, at, regs, done));
                }
                max
            }
            Op::Sub(a, b) => self.get(*a, at, regs, done) - self.get(*b, at, regs, done),
            Op::Mix(a, b, t) => {
                let t = self.get(*t, at, regs, done).clamp(0.0, 1.0);
                if t <= 0.0 {
                    self.get(*a, at, regs, done)
                } else if t >= 1.0 {
                    self.get(*b, at, regs, done)
                } else {
                    let (a, b) = (self.get(*a, at, regs, done), self.get(*b, at, regs, done));
                    a + (b - a) * t
                }
            }
            Op::Clamp(input, min, max) => self.get(*input, at, regs, done).max(*min).min(*max),
            Op::Abs(input) => self.get(*input, at, regs, done).abs(),
            Op::Pow(input, exponent) => {
                // `powf` es de lo más caro del grafo: se evita en los casos corrientes.
                let base = self.get(*input, at, regs, done).max(0.0);
                if base == 0.0 || base == 1.0 {
                    base
                } else if *exponent == 2.0 {
                    base * base
                } else if *exponent == 1.5 {
                    base * base.sqrt()
                } else if *exponent == 0.5 {
                    base.sqrt()
                } else {
                    base.powf(*exponent)
                }
            }
            Op::Smoothstep(input, from, to) => smoothstep(*from, *to, self.get(*input, at, regs, done)),
            Op::Curve(input, points, smooth) => curve(points, *smooth, self.get(*input, at, regs, done)),
            Op::Terrace(input, step, sharpness) => {
                let value = self.get(*input, at, regs, done) / step;
                let (floor, t) = (value.floor(), value - value.floor());
                let half = 0.5 * (1.0 - sharpness);
                let t = if half >= 0.5 { t } else { smoothstep(0.5 - half - 1e-4, 0.5 + half + 1e-4, t) };
                (floor + t) * step
            }
        };
        regs[index] = value;
        done[index / 64] |= 1 << (index % 64);
        value
    }
}

fn warped(at: At, warp: f32, warp_inv: f64, seed: u32) -> (f64, f64) {
    if warp == 0.0 {
        return (at.x, at.z);
    }
    let (x, z) = (at.x * warp_inv, at.z * warp_inv);
    (
        at.x + (perlin2(x, z, seed ^ 0x1B56) * warp) as f64,
        at.z + (perlin2(x, z, seed ^ 0x7C3D) * warp) as f64,
    )
}

fn curve(points: &[[f32; 2]], smooth: bool, x: f32) -> f32 {
    let (first, last) = (points[0], points[points.len() - 1]);
    if x <= first[0] {
        return first[1];
    }
    if x >= last[0] {
        return last[1];
    }
    let next = points.iter().position(|point| point[0] > x).unwrap_or(points.len() - 1);
    let (a, b) = (points[next - 1], points[next]);
    let mut t = (x - a[0]) / (b[0] - a[0]).max(1e-6);
    if smooth {
        t = t * t * (3.0 - 2.0 * t);
    }
    a[1] + (b[1] - a[1]) * t
}

fn unit(hash: u32) -> f32 {
    (hash & 0xFFFF) as f32 / 65535.0
}

/// Centro de la celda (cx, cz), en unidades de celda.
fn cell_point(cx: i32, cz: i32, jitter: f32, seed: u32) -> (f32, f32) {
    let hash = hash2(cx, cz, seed);
    (0.5 + (unit(hash) - 0.5) * jitter, 0.5 + (unit(hash >> 16) - 0.5) * jitter)
}

/// Ruido de celdas en unidades de celda (salvo `Value`, que va de 0 a 1).
fn cells(x: f64, z: f64, mode: CellMode, jitter: f32, seed: u32) -> f32 {
    let (cx, cz) = (x.floor(), z.floor());
    let (fx, fz) = ((x - cx) as f32, (z - cz) as f32);
    let (cx, cz) = (cx as i64 as i32, cz as i64 as i32);
    // Vector desde el punto hasta el centro de cada celda vecina.
    let mut to = [(0.0f32, 0.0f32); 9];
    let (mut nearest, mut best) = (0, f32::INFINITY);
    for (index, slot) in to.iter_mut().enumerate() {
        let (dx, dz) = (index as i32 % 3 - 1, index as i32 / 3 - 1);
        let (px, pz) = cell_point(cx.wrapping_add(dx), cz.wrapping_add(dz), jitter, seed);
        *slot = (dx as f32 + px - fx, dz as f32 + pz - fz);
        let distance = slot.0 * slot.0 + slot.1 * slot.1;
        if distance < best {
            (nearest, best) = (index, distance);
        }
    }
    match mode {
        CellMode::Center => best.sqrt(),
        CellMode::Value => {
            let (dx, dz) = (nearest as i32 % 3 - 1, nearest as i32 / 3 - 1);
            unit(hash2(cx.wrapping_add(dx), cz.wrapping_add(dz), seed ^ 0xB10C))
        }
        CellMode::Edge => {
            // Distancia a la mediatriz con cada vecino: la menor es la del borde.
            let a = to[nearest];
            let mut edge = f32::INFINITY;
            for (index, b) in to.iter().enumerate() {
                if index == nearest {
                    continue;
                }
                let (dx, dz) = (b.0 - a.0, b.1 - a.1);
                let length = (dx * dx + dz * dz).sqrt().max(1e-6);
                edge = edge.min(((a.0 + b.0) * 0.5 * dx + (a.1 + b.1) * 0.5 * dz) / length);
            }
            edge.max(0.0)
        }
    }
}

fn spots(x: f64, z: f64, spacing: f64, radius: f32, chance: f32, vary: f32, seed: u32) -> f32 {
    let (gx, gz) = ((x / spacing).floor(), (z / spacing).floor());
    let (fx, fz) = ((x / spacing - gx) as f32, (z / spacing - gz) as f32);
    let (gx, gz) = (gx as i64 as i32, gz as i64 as i32);
    let spacing = spacing as f32;
    let mut value = 0.0f32;
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (cx, cz) = (gx.wrapping_add(dx), gz.wrapping_add(dz));
            let hash = hash2(cx, cz, seed);
            if unit(hash2(cx, cz, seed ^ 0xC4A2)) > chance {
                continue;
            }
            let own = radius * (1.0 - vary * unit(hash2(cx, cz, seed ^ 0x51DE)));
            let (px, pz) = (dx as f32 + unit(hash) - fx, dz as f32 + unit(hash >> 16) - fz);
            let distance = (px * px + pz * pz).sqrt() * spacing;
            value = value.max(1.0 - distance / own);
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(text: &str) -> BTreeMap<String, NodeDef> {
        #[derive(Deserialize)]
        struct File {
            nodes: BTreeMap<String, NodeDef>,
        }
        toml::from_str::<File>(text).expect("valid graph").nodes
    }

    fn eval(text: &str, root: &str, x: f64, z: f64) -> f32 {
        let (program, regs) = Program::compile(&nodes(text), &[root], 7).expect("compiles");
        let mut values = [0.0; MAX_OPS];
        program.eval(At { x, z, u: 0.25, v: 0.75 }, &mut values);
        values[regs[0] as usize]
    }

    #[test]
    fn math_nodes_combine_inputs_and_numbers() {
        let graph = r#"
            [nodes.base]
            type = "constant"
            value = 10
            [nodes.sum]
            type = "add"
            inputs = ["base", 5, "u"]
            [nodes.out]
            type = "mix"
            a = "sum"
            b = 0
            t = 0.5
        "#;
        assert!((eval(graph, "out", 0.0, 0.0) - 7.625).abs() < 1e-5);
    }

    #[test]
    fn curves_interpolate_and_clamp() {
        let graph = r#"
            [nodes.out]
            type = "curve"
            input = "x"
            points = [[0, 0], [10, 100], [20, 50]]
        "#;
        assert_eq!(eval(graph, "out", -5.0, 0.0), 0.0);
        assert_eq!(eval(graph, "out", 5.0, 0.0), 50.0);
        assert_eq!(eval(graph, "out", 15.0, 0.0), 75.0);
        assert_eq!(eval(graph, "out", 99.0, 0.0), 50.0);
    }

    #[test]
    fn terraces_are_flat_between_steps() {
        let graph = r#"
            [nodes.out]
            type = "terrace"
            input = "x"
            step = 10
            sharpness = 1
        "#;
        assert!((eval(graph, "out", 12.0, 0.0) - 10.0).abs() < 0.01);
        assert!((eval(graph, "out", 18.0, 0.0) - 20.0).abs() < 0.01);
    }

    #[test]
    fn noise_is_deterministic_bounded_and_named() {
        let graph = r#"
            [nodes.a]
            type = "noise"
            scale = 100
            [nodes.b]
            type = "noise"
            scale = 100
            [nodes.ridge]
            type = "ridge"
            scale = [50, 200]
            warp = 30
        "#;
        let mut differs = false;
        for step in 0..200 {
            let (x, z) = (step as f64 * 13.7, step as f64 * -7.3);
            let a = eval(graph, "a", x, z);
            assert_eq!(a, eval(graph, "a", x, z));
            assert!((-1.01..=1.01).contains(&a));
            differs |= (a - eval(graph, "b", x, z)).abs() > 0.05;
            assert!((0.0..=1.0).contains(&eval(graph, "ridge", x, z)));
        }
        assert!(differs, "nodes with different names use different noise");
    }

    #[test]
    fn cells_have_edges_and_per_cell_values() {
        let graph = r#"
            [nodes.edge]
            type = "cells"
            scale = 40
            [nodes.value]
            type = "cells"
            scale = 40
            mode = "value"
            seed = 3
        "#;
        let (mut near_edge, mut far) = (false, false);
        let mut values = std::collections::BTreeSet::new();
        for step in 0..400 {
            let x = step as f64 * 0.9;
            let edge = eval(graph, "edge", x, 11.0);
            assert!((0.0..40.0).contains(&edge));
            near_edge |= edge < 1.0;
            far |= edge > 8.0;
            values.insert((eval(graph, "value", x, 11.0) * 1000.0) as i32);
        }
        assert!(near_edge && far);
        // 360 m de recorrido cruzan unas cuantas celdas, y dentro de cada una no cambia.
        assert!((4..40).contains(&values.len()), "{} values", values.len());
    }

    #[test]
    fn spots_peak_at_one_and_fade_out() {
        let graph = r#"
            [nodes.out]
            type = "spots"
            spacing = 100
            radius = 40
        "#;
        let samples: Vec<f32> =
            (0..4000).map(|i| eval(graph, "out", (i % 200) as f64 * 2.5, (i / 200) as f64 * 25.0)).collect();
        assert!(samples.iter().all(|v| *v <= 1.0));
        assert!(samples.iter().any(|v| *v > 0.8));
        assert!(samples.iter().filter(|v| **v <= 0.0).count() > samples.len() / 4);
    }

    #[test]
    fn every_node_type_has_a_schema_that_builds() {
        let kinds = [
            "constant", "noise", "ridge", "cells", "spots", "point", "add", "mul", "min", "max", "sub", "mix",
            "clamp", "abs", "pow", "smoothstep", "curve", "terrace",
        ];
        assert_eq!(NODE_TYPES.len(), kinds.len());
        for kind in kinds {
            let schema = schema(kind).unwrap_or_else(|| panic!("no schema for {kind}"));
            let node: NodeDef =
                toml::Value::Table(schema.new_node()).try_into().unwrap_or_else(|e| panic!("{kind}: {e}"));
            let nodes = BTreeMap::from([("n".to_string(), node)]);
            Program::compile(&nodes, &["n"], 1).unwrap_or_else(|e| panic!("{kind}: {e:#}"));
            // Cada parámetro opcional se acepta.
            for param in schema.params.iter().filter(|param| !param.required) {
                let mut table = schema.new_node();
                let value = match param.kind {
                    ParamKind::Choice(options) => toml::Value::String(options[1].into()),
                    ParamKind::Bool => toml::Value::Boolean(true),
                    ParamKind::Seed | ParamKind::Integer => toml::Value::Integer(3),
                    _ => toml::Value::Float(param.default.max(1.0) as f64),
                };
                table.insert(param.name.into(), value);
                let parsed: Result<NodeDef, _> = toml::Value::Table(table).try_into();
                assert!(parsed.is_ok(), "{kind}.{}: {parsed:?}", param.name);
            }
        }
    }

    #[test]
    fn mistakes_are_reported() {
        let compile = |text: &str, root: &str| {
            Program::compile(&nodes(text), &[root], 1).map(|_| ()).map_err(|e| format!("{e:#}"))
        };
        let cycle = r#"
            [nodes.a]
            type = "abs"
            input = "b"
            [nodes.b]
            type = "abs"
            input = "a"
        "#;
        assert!(compile(cycle, "a").unwrap_err().contains("depends on itself"));
        let missing = r#"
            [nodes.a]
            type = "abs"
            input = "nope"
        "#;
        assert!(compile(missing, "a").unwrap_err().contains("unknown node \"nope\""));
        assert!(compile(missing, "other").unwrap_err().contains("unknown node \"other\""));
        let typo = "[nodes.a]\ntype = \"noise\"\nscale = 10\noctavs = 2\n";
        assert!(toml::from_str::<toml::Table>(typo).is_ok());
        #[derive(Deserialize)]
        struct File {
            #[allow(dead_code)]
            nodes: BTreeMap<String, NodeDef>,
        }
        assert!(toml::from_str::<File>(typo).is_err(), "unknown parameters are rejected");
    }
}
