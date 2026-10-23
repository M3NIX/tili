use crate::tiling::{LayoutError, Orientation, PaneId, TilingTree};

pub const WORKSPACE_COUNT: usize = 10;

#[derive(Clone, Debug, Default)]
pub struct Workspace {
    pub layout: TilingTree,
    pub fullscreen: Option<PaneId>,
}

impl Workspace {
    pub fn with_pane(pane: PaneId) -> Self {
        Self {
            layout: TilingTree::new(pane),
            fullscreen: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Workspaces {
    items: Vec<Workspace>,
    active: usize,
    history: Vec<usize>,
}

impl Workspaces {
    pub fn new(initial_pane: PaneId) -> Self {
        let mut items = vec![Workspace::default(); WORKSPACE_COUNT];
        items[0] = Workspace::with_pane(initial_pane);
        Self {
            items,
            active: 0,
            history: Vec::new(),
        }
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> &Workspace {
        &self.items[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Workspace {
        &mut self.items[self.active]
    }

    pub fn get(&self, index: usize) -> Option<&Workspace> {
        self.items.get(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut Workspace> {
        self.items.get_mut(index)
    }

    pub fn workspace_containing(&self, pane: PaneId) -> Option<usize> {
        self.items
            .iter()
            .position(|workspace| workspace.layout.contains(pane))
    }

    pub fn open_count(&self) -> usize {
        self.items
            .iter()
            .filter(|workspace| !workspace.layout.is_empty())
            .count()
    }

    pub fn open_map(&self) -> [bool; WORKSPACE_COUNT] {
        std::array::from_fn(|index| !self.items[index].layout.is_empty())
    }

    pub fn switch(&mut self, index: usize) -> bool {
        if index >= self.items.len() || index == self.active {
            return false;
        }
        self.history.retain(|visited| *visited != self.active);
        self.history.push(self.active);
        self.active = index;
        true
    }

    pub fn switch_to_last_open(&mut self) -> Option<usize> {
        while let Some(index) = self.history.pop() {
            if index != self.active && !self.items[index].layout.is_empty() {
                self.active = index;
                return Some(index);
            }
        }
        let index = self
            .items
            .iter()
            .enumerate()
            .find(|(index, workspace)| *index != self.active && !workspace.layout.is_empty())
            .map(|(index, _)| index)?;
        self.active = index;
        Some(index)
    }

    pub fn move_focused_to(
        &mut self,
        target: usize,
        orientation: Orientation,
    ) -> Result<Option<PaneId>, LayoutError> {
        if target >= self.items.len() || target == self.active {
            return Ok(None);
        }
        let source_index = self.active;
        let focused = self.items[source_index]
            .layout
            .focused()
            .ok_or(LayoutError::NoFocusedPane)?;
        let (source, destination) = two_mut(&mut self.items, source_index, target);

        source.layout.close(focused)?;
        if source.fullscreen == Some(focused) {
            source.fullscreen = None;
        }
        if destination.layout.is_empty() {
            destination.layout = TilingTree::new(focused);
        } else {
            destination.layout.split_focused(focused, orientation)?;
        }
        destination.fullscreen = None;
        Ok(Some(focused))
    }
}

fn two_mut<T>(items: &mut [T], first: usize, second: usize) -> (&mut T, &mut T) {
    debug_assert_ne!(first, second);
    if first < second {
        let (left, right) = items.split_at_mut(second);
        (&mut left[first], &mut right[0])
    } else {
        let (left, right) = items.split_at_mut(first);
        (&mut right[0], &mut left[second])
    }
}
