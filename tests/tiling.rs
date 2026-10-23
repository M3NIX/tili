use tili::tiling::{
    Direction, LayoutError, MinimumSize, Orientation, PaneId, Rect, Resize, TilingTree,
};

const BOUNDS: Rect = Rect::new(0, 0, 120, 40);
const MINIMUM: MinimumSize = MinimumSize::new(10, 5);

fn geometry(tree: &TilingTree, pane: u64) -> Rect {
    tree.layout(BOUNDS, MINIMUM)
        .into_iter()
        .find(|geometry| geometry.pane == PaneId(pane))
        .expect("pane should have geometry")
        .rect
}

#[test]
fn splitting_the_focused_pane_builds_a_binary_tree_and_focuses_the_new_pane() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    tree.split_focused(PaneId(3), Orientation::Vertical)
        .unwrap();

    assert_eq!(tree.pane_count(), 3);
    assert_eq!(tree.focused(), Some(PaneId(3)));
    assert_eq!(tree.root_orientation(), Some(Orientation::Horizontal));
    assert_eq!(tree.root_ratio(), Some(0.5));
    assert!(tree.contains(PaneId(1)));
    assert!(tree.contains(PaneId(2)));
    assert!(tree.contains(PaneId(3)));
}

#[test]
fn same_orientation_panes_share_the_container_equally_like_i3() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    assert_eq!(geometry(&tree, 1).width, 60);
    assert_eq!(geometry(&tree, 2).width, 60);

    tree.split_focused(PaneId(3), Orientation::Horizontal)
        .unwrap();
    assert_eq!(geometry(&tree, 1), Rect::new(0, 0, 40, 40));
    assert_eq!(geometry(&tree, 2), Rect::new(40, 0, 40, 40));
    assert_eq!(geometry(&tree, 3), Rect::new(80, 0, 40, 40));

    tree.focus(PaneId(2)).unwrap();
    tree.split_focused(PaneId(4), Orientation::Horizontal)
        .unwrap();
    assert_eq!(geometry(&tree, 1), Rect::new(0, 0, 30, 40));
    assert_eq!(geometry(&tree, 2), Rect::new(30, 0, 30, 40));
    assert_eq!(geometry(&tree, 4), Rect::new(60, 0, 30, 40));
    assert_eq!(geometry(&tree, 3), Rect::new(90, 0, 30, 40));
}

#[test]
fn closing_an_equal_sibling_redistributes_the_remaining_space() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    tree.split_focused(PaneId(3), Orientation::Horizontal)
        .unwrap();
    tree.close(PaneId(2)).unwrap();

    assert_eq!(geometry(&tree, 1), Rect::new(0, 0, 60, 40));
    assert_eq!(geometry(&tree, 3), Rect::new(60, 0, 60, 40));
}

#[test]
fn different_orientation_splits_remain_nested() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    tree.split_focused(PaneId(3), Orientation::Vertical)
        .unwrap();

    assert_eq!(geometry(&tree, 1), Rect::new(0, 0, 60, 40));
    assert_eq!(geometry(&tree, 2), Rect::new(60, 0, 60, 20));
    assert_eq!(geometry(&tree, 3), Rect::new(60, 20, 60, 20));
}

#[test]
fn duplicate_pane_ids_are_rejected() {
    let mut tree = TilingTree::new(PaneId(1));

    assert_eq!(
        tree.split_focused(PaneId(1), Orientation::Horizontal),
        Err(LayoutError::DuplicatePane(PaneId(1)))
    );
}

#[test]
fn replacing_a_pane_id_preserves_the_tree_and_focus() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    let before = tree.layout(Rect::new(0, 0, 80, 24), MinimumSize::new(3, 3));

    tree.replace_pane(PaneId(2), PaneId(7)).unwrap();

    assert_eq!(tree.focused(), Some(PaneId(7)));
    assert!(!tree.contains(PaneId(2)));
    assert!(tree.contains(PaneId(7)));
    let after = tree.layout(Rect::new(0, 0, 80, 24), MinimumSize::new(3, 3));
    assert_eq!(before[0], after[0]);
    assert_eq!(before[1].rect, after[1].rect);
    assert_eq!(after[1].pane, PaneId(7));
}

#[test]
fn closing_a_pane_collapses_its_parent_and_focuses_the_sibling() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    tree.split_focused(PaneId(3), Orientation::Vertical)
        .unwrap();

    tree.close_focused().unwrap();

    assert_eq!(tree.pane_count(), 2);
    assert_eq!(tree.focused(), Some(PaneId(2)));
    assert!(!tree.contains(PaneId(3)));
    assert_eq!(geometry(&tree, 2), Rect::new(60, 0, 60, 40));
}

#[test]
fn removing_the_final_pane_leaves_a_valid_empty_tree() {
    let mut tree = TilingTree::new(PaneId(7));

    assert_eq!(tree.close_focused().unwrap(), None);
    assert!(tree.is_empty());
    assert_eq!(tree.focused(), None);
    assert!(tree.layout(BOUNDS, MINIMUM).is_empty());

    tree.add_initial(PaneId(8)).unwrap();
    assert_eq!(tree.focused(), Some(PaneId(8)));
}

#[test]
fn geometry_honors_split_orientation_and_ratio() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Vertical)
        .unwrap();
    tree.focus(PaneId(1)).unwrap();
    assert!(tree.resize_focused(Resize::ShrinkHeight, 10, BOUNDS, MINIMUM));

    assert_eq!(geometry(&tree, 1), Rect::new(0, 0, 120, 10));
    assert_eq!(geometry(&tree, 2), Rect::new(0, 10, 120, 30));
    assert_eq!(tree.root_ratio(), Some(0.25));
}

#[test]
fn layout_enforces_recursive_minimum_sizes_when_space_allows() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    tree.split_focused(PaneId(3), Orientation::Horizontal)
        .unwrap();
    tree.focus(PaneId(1)).unwrap();
    assert!(tree.resize_focused(Resize::GrowWidth, 100, BOUNDS, MINIMUM));

    assert_eq!(geometry(&tree, 1).width, 100);
    assert_eq!(geometry(&tree, 2).width, 10);
    assert_eq!(geometry(&tree, 3).width, 10);
}

#[test]
fn tiny_layouts_degrade_without_underflow_or_panics() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    tree.split_focused(PaneId(3), Orientation::Horizontal)
        .unwrap();

    let layout = tree.layout(Rect::new(u16::MAX - 1, 0, 2, 1), MINIMUM);
    assert_eq!(layout.len(), 3);
    assert_eq!(layout.iter().map(|item| item.rect.width).sum::<u16>(), 2);
}

#[test]
fn directional_focus_uses_geometry_and_edge_overlap() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    tree.split_focused(PaneId(3), Orientation::Vertical)
        .unwrap();
    tree.focus(PaneId(1)).unwrap();

    assert_eq!(
        tree.focus_direction(Direction::Right, BOUNDS, MINIMUM),
        Some(PaneId(2))
    );
    assert_eq!(
        tree.focus_direction(Direction::Down, BOUNDS, MINIMUM),
        Some(PaneId(3))
    );
    assert_eq!(
        tree.focus_direction(Direction::Left, BOUNDS, MINIMUM),
        Some(PaneId(1))
    );
    assert_eq!(tree.focus_direction(Direction::Up, BOUNDS, MINIMUM), None);
}

#[test]
fn moving_a_pane_swaps_nearest_leaves_and_keeps_focus_on_the_moved_pane() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    let before = geometry(&tree, 2);

    assert!(tree.move_focused(Direction::Left, BOUNDS, MINIMUM));
    assert_eq!(tree.focused(), Some(PaneId(2)));
    assert_eq!(geometry(&tree, 2), Rect::new(0, 0, 60, 40));
    assert_eq!(geometry(&tree, 1), before);
}

#[test]
fn resizing_adjusts_the_nearest_matching_ancestor_by_cells() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    tree.split_focused(PaneId(3), Orientation::Vertical)
        .unwrap();

    assert!(tree.resize_focused(Resize::GrowHeight, 10, BOUNDS, MINIMUM));
    assert_eq!(geometry(&tree, 3).height, 30);
    assert_eq!(geometry(&tree, 2).height, 10);

    assert!(tree.resize_focused(Resize::GrowWidth, 10, BOUNDS, MINIMUM));
    assert_eq!(geometry(&tree, 3).width, 70);
    assert_eq!(geometry(&tree, 1).width, 50);
}

#[test]
fn resizing_stops_at_minimum_subtree_dimensions() {
    let mut tree = TilingTree::new(PaneId(1));
    tree.split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();

    assert!(tree.resize_focused(Resize::GrowWidth, 100, BOUNDS, MINIMUM));
    assert_eq!(geometry(&tree, 1).width, 10);
    assert_eq!(geometry(&tree, 2).width, 110);
    assert!(!tree.resize_focused(Resize::GrowWidth, 10, BOUNDS, MINIMUM));
}
