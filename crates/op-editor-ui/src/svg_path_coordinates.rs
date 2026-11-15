//! Lexical mapping for one actual path draw, including its paint/clip/stroke.
use jian_scene::path_coordinates::ScenePathGeometry;
use std::cell::Cell;
thread_local! { static CURRENT: Cell<Option<[f32;4]>> = const { Cell::new(None) }; }
pub fn current() -> Option<[f32; 4]> {
    CURRENT.with(Cell::get)
}
#[must_use]
pub struct Scope(Option<[f32; 4]>);
impl Scope {
    pub fn new(geometry: Option<&ScenePathGeometry>) -> Self {
        Self(CURRENT.with(|current| current.replace(geometry.map(|g| g.view_box))))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|current| current.set(self.0));
    }
}
