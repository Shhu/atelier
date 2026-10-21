//! Vector paths and shapes: the geometry of paths (Pen, Paths panel), their
//! rasterization, the shape layers' content, and the tracing of a
//! selection into a path.

pub mod geom;
pub mod raster;
pub mod shape;
pub mod trace;

pub use geom::{Anchor, Bounds, Path, PathOp, Pt, SubPath, pt};
pub use raster::{Cap, Join};
pub use shape::{Align, Custom, Live, Paint, Shape, ShapeKind, ShapeStroke};
