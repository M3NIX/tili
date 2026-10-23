use tili::tiling::{Orientation, PaneId};
use tili::workspace::{WORKSPACE_COUNT, Workspaces};

#[test]
fn switching_restores_each_workspaces_layout_and_focus() {
    let mut workspaces = Workspaces::new(PaneId(1));
    workspaces
        .active_mut()
        .layout
        .split_focused(PaneId(2), Orientation::Horizontal)
        .unwrap();
    workspaces.switch(1);
    assert!(workspaces.active().layout.is_empty());
    workspaces.active_mut().layout = tili::tiling::TilingTree::new(PaneId(3));

    workspaces.switch(0);
    assert_eq!(workspaces.active().layout.pane_count(), 2);
    assert_eq!(workspaces.active().layout.focused(), Some(PaneId(2)));
    assert_eq!(WORKSPACE_COUNT, 10);
}

#[test]
fn moving_a_pane_preserves_it_and_focuses_it_on_the_destination() {
    let mut workspaces = Workspaces::new(PaneId(1));
    workspaces
        .active_mut()
        .layout
        .split_focused(PaneId(2), Orientation::Vertical)
        .unwrap();

    assert_eq!(
        workspaces
            .move_focused_to(2, Orientation::Horizontal)
            .unwrap(),
        Some(PaneId(2))
    );
    assert_eq!(workspaces.active().layout.pane_count(), 1);
    assert_eq!(workspaces.get(2).unwrap().layout.focused(), Some(PaneId(2)));
    assert!(workspaces.get(2).unwrap().layout.contains(PaneId(2)));
}

#[test]
fn moving_the_last_pane_leaves_a_valid_empty_workspace() {
    let mut workspaces = Workspaces::new(PaneId(1));
    workspaces
        .move_focused_to(9, Orientation::Horizontal)
        .unwrap();
    assert!(workspaces.active().layout.is_empty());
    assert!(workspaces.get(9).unwrap().layout.contains(PaneId(1)));
}

#[test]
fn closing_workspace_can_return_to_the_most_recent_open_workspace() {
    let mut workspaces = Workspaces::new(PaneId(1));
    workspaces.switch(1);
    workspaces.active_mut().layout = tili::tiling::TilingTree::new(PaneId(2));
    workspaces.switch(2);
    workspaces.active_mut().layout = tili::tiling::TilingTree::new(PaneId(3));
    workspaces.active_mut().layout.close(PaneId(3)).unwrap();

    assert_eq!(workspaces.open_count(), 2);
    assert_eq!(workspaces.switch_to_last_open(), Some(1));
    assert_eq!(workspaces.active_index(), 1);
}
