use tili::tiling::{MinimumSize, Orientation, PaneId, Rect, TilingTree};

fn main() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .expect("example split must succeed");
    tree.split_focused(PaneId(3), Orientation::Vertical)
        .expect("example split must succeed");

    for geometry in tree.layout(Rect::new(0, 0, 80, 24), MinimumSize::new(3, 3)) {
        println!(
            "pane {}: x={} y={} width={} height={}",
            geometry.pane,
            geometry.rect.x,
            geometry.rect.y,
            geometry.rect.width,
            geometry.rect.height
        );
    }
}
