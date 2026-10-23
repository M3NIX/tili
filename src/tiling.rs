use std::cmp::Ordering;
use std::fmt;

const DEFAULT_RATIO: f64 = 0.5;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PaneId(pub u64);

impl fmt::Display for PaneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Orientation {
    /// Children are placed side by side.
    Horizontal,
    /// Children are placed one above the other.
    Vertical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resize {
    GrowWidth,
    ShrinkWidth,
    GrowHeight,
    ShrinkHeight,
}

impl Resize {
    fn orientation(self) -> Orientation {
        match self {
            Self::GrowWidth | Self::ShrinkWidth => Orientation::Horizontal,
            Self::GrowHeight | Self::ShrinkHeight => Orientation::Vertical,
        }
    }

    fn signed_cells(self, cells: u16) -> i32 {
        let cells = i32::from(cells);
        match self {
            Self::GrowWidth | Self::GrowHeight => cells,
            Self::ShrinkWidth | Self::ShrinkHeight => -cells,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains_point(self, x: u16, y: u16) -> bool {
        u32::from(x) >= u32::from(self.x)
            && u32::from(x) < self.right()
            && u32::from(y) >= u32::from(self.y)
            && u32::from(y) < self.bottom()
    }

    fn right(self) -> u32 {
        u32::from(self.x) + u32::from(self.width)
    }

    fn bottom(self) -> u32 {
        u32::from(self.y) + u32::from(self.height)
    }

    fn center_x_twice(self) -> u32 {
        u32::from(self.x) * 2 + u32::from(self.width)
    }

    fn center_y_twice(self) -> u32 {
        u32::from(self.y) * 2 + u32::from(self.height)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MinimumSize {
    pub width: u16,
    pub height: u16,
}

impl MinimumSize {
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

impl Default for MinimumSize {
    fn default() -> Self {
        Self::new(3, 2)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneGeometry {
    pub pane: PaneId,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Pane(PaneId),
    Split {
        orientation: Orientation,
        ratio: f64,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    fn pane(pane: PaneId) -> Self {
        Self::Pane(pane)
    }

    fn split(orientation: Orientation, ratio: f64, first: Node, second: Node) -> Self {
        Self::Split {
            orientation,
            ratio: sanitize_ratio(ratio),
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn contains(&self, pane: PaneId) -> bool {
        match self {
            Self::Pane(candidate) => *candidate == pane,
            Self::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    fn pane_count(&self) -> usize {
        match self {
            Self::Pane(_) => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    fn ratio(&self) -> Option<f64> {
        match self {
            Self::Pane(_) => None,
            Self::Split { ratio, .. } => Some(*ratio),
        }
    }

    fn orientation(&self) -> Option<Orientation> {
        match self {
            Self::Pane(_) => None,
            Self::Split { orientation, .. } => Some(*orientation),
        }
    }

    fn first_pane(&self) -> PaneId {
        match self {
            Self::Pane(pane) => *pane,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    fn required_size(&self, minimum: MinimumSize) -> RequiredSize {
        match self {
            Self::Pane(_) => RequiredSize {
                width: u32::from(minimum.width.max(1)),
                height: u32::from(minimum.height.max(1)),
            },
            Self::Split {
                orientation,
                first,
                second,
                ..
            } => {
                let first = first.required_size(minimum);
                let second = second.required_size(minimum);
                match orientation {
                    Orientation::Horizontal => RequiredSize {
                        width: first.width.saturating_add(second.width),
                        height: first.height.max(second.height),
                    },
                    Orientation::Vertical => RequiredSize {
                        width: first.width.max(second.width),
                        height: first.height.saturating_add(second.height),
                    },
                }
            }
        }
    }

    fn collect_layout(&self, rect: Rect, minimum: MinimumSize, output: &mut Vec<PaneGeometry>) {
        match self {
            Self::Pane(pane) => output.push(PaneGeometry { pane: *pane, rect }),
            Self::Split {
                orientation,
                ratio,
                first,
                second,
            } => {
                let first_required = first.required_size(minimum);
                let second_required = second.required_size(minimum);
                let (first_rect, second_rect) =
                    split_rect(rect, *orientation, *ratio, first_required, second_required);
                first.collect_layout(first_rect, minimum, output);
                second.collect_layout(second_rect, minimum, output);
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    DuplicatePane(PaneId),
    NoFocusedPane,
    PaneNotFound(PaneId),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicatePane(pane) => write!(formatter, "pane {pane} already exists"),
            Self::NoFocusedPane => write!(formatter, "the layout has no focused pane"),
            Self::PaneNotFound(pane) => write!(formatter, "pane {pane} does not exist"),
        }
    }
}

impl std::error::Error for LayoutError {}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TilingTree {
    root: Option<Node>,
    focused: Option<PaneId>,
}

impl TilingTree {
    pub fn new(initial: PaneId) -> Self {
        Self {
            root: Some(Node::pane(initial)),
            focused: Some(initial),
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn root_orientation(&self) -> Option<Orientation> {
        self.root.as_ref().and_then(Node::orientation)
    }

    pub fn root_ratio(&self) -> Option<f64> {
        self.root.as_ref().and_then(Node::ratio)
    }

    pub fn focused(&self) -> Option<PaneId> {
        self.focused
    }

    pub fn pane_count(&self) -> usize {
        self.root.as_ref().map_or(0, Node::pane_count)
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.as_ref().is_some_and(|root| root.contains(pane))
    }

    pub fn add_initial(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        if self.contains(pane) {
            return Err(LayoutError::DuplicatePane(pane));
        }
        if self.root.is_some() {
            return Err(LayoutError::NoFocusedPane);
        }
        self.root = Some(Node::pane(pane));
        self.focused = Some(pane);
        Ok(())
    }

    pub fn focus(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        if !self.contains(pane) {
            return Err(LayoutError::PaneNotFound(pane));
        }
        self.focused = Some(pane);
        Ok(())
    }

    pub fn replace_pane(&mut self, old: PaneId, new: PaneId) -> Result<(), LayoutError> {
        if self.contains(new) {
            return Err(LayoutError::DuplicatePane(new));
        }
        let root = self.root.as_mut().ok_or(LayoutError::PaneNotFound(old))?;
        if !replace_pane_id(root, old, new) {
            return Err(LayoutError::PaneNotFound(old));
        }
        if self.focused == Some(old) {
            self.focused = Some(new);
        }
        Ok(())
    }

    pub fn split_focused(
        &mut self,
        new_pane: PaneId,
        orientation: Orientation,
    ) -> Result<(), LayoutError> {
        if self.contains(new_pane) {
            return Err(LayoutError::DuplicatePane(new_pane));
        }
        let focused = self.focused.ok_or(LayoutError::NoFocusedPane)?;
        let root = self.root.as_mut().ok_or(LayoutError::NoFocusedPane)?;
        if !split_leaf(root, focused, new_pane, orientation) {
            return Err(LayoutError::PaneNotFound(focused));
        }
        rebalance_component_containing(root, new_pane, orientation);
        self.focused = Some(new_pane);
        Ok(())
    }

    pub fn close_focused(&mut self) -> Result<Option<PaneId>, LayoutError> {
        let focused = self.focused.ok_or(LayoutError::NoFocusedPane)?;
        self.close(focused)?;
        Ok(self.focused)
    }

    pub fn close(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        if !self.contains(pane) {
            return Err(LayoutError::PaneNotFound(pane));
        }

        let was_focused = self.focused == Some(pane);
        let root = self.root.take().expect("contains pane requires a root");
        let (new_root, replacement_focus, collapsed_orientation) = remove_pane(root, pane);
        self.root = new_root;
        if let (Some(root), Some(replacement), Some(orientation)) =
            (self.root.as_mut(), replacement_focus, collapsed_orientation)
        {
            rebalance_component_containing(root, replacement, orientation);
        }
        if was_focused {
            self.focused = replacement_focus.or_else(|| self.root.as_ref().map(Node::first_pane));
        }
        if self.root.is_none() {
            self.focused = None;
        }
        Ok(())
    }

    pub fn layout(&self, bounds: Rect, minimum: MinimumSize) -> Vec<PaneGeometry> {
        let mut output = Vec::with_capacity(self.pane_count());
        if let Some(root) = &self.root {
            root.collect_layout(bounds, minimum, &mut output);
        }
        output
    }

    pub fn focus_direction(
        &mut self,
        direction: Direction,
        bounds: Rect,
        minimum: MinimumSize,
    ) -> Option<PaneId> {
        let next = self.neighbor(direction, bounds, minimum)?;
        self.focused = Some(next);
        Some(next)
    }

    pub fn neighbor(
        &self,
        direction: Direction,
        bounds: Rect,
        minimum: MinimumSize,
    ) -> Option<PaneId> {
        let focused = self.focused?;
        let layout = self.layout(bounds, minimum);
        let current = layout.iter().find(|item| item.pane == focused)?.rect;

        layout
            .iter()
            .filter(|candidate| candidate.pane != focused)
            .filter_map(|candidate| {
                directional_score(current, candidate.rect, direction)
                    .map(|score| (score, candidate.pane))
            })
            .min_by(|(left_score, left_id), (right_score, right_id)| {
                compare_scores(*left_score, *right_score).then_with(|| left_id.cmp(right_id))
            })
            .map(|(_, pane)| pane)
    }

    pub fn move_focused(
        &mut self,
        direction: Direction,
        bounds: Rect,
        minimum: MinimumSize,
    ) -> bool {
        let Some(focused) = self.focused else {
            return false;
        };
        let Some(neighbor) = self.neighbor(direction, bounds, minimum) else {
            return false;
        };
        let Some(root) = &mut self.root else {
            return false;
        };
        swap_panes(root, focused, neighbor);
        true
    }

    pub fn resize_focused(
        &mut self,
        resize: Resize,
        cells: u16,
        bounds: Rect,
        minimum: MinimumSize,
    ) -> bool {
        if cells == 0 {
            return false;
        }
        let Some(focused) = self.focused else {
            return false;
        };
        let Some(root) = &mut self.root else {
            return false;
        };
        resize_node(
            root,
            focused,
            resize.orientation(),
            resize.signed_cells(cells),
            bounds,
            minimum,
        )
    }
}

#[derive(Clone, Copy)]
struct RequiredSize {
    width: u32,
    height: u32,
}

fn sanitize_ratio(ratio: f64) -> f64 {
    if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        DEFAULT_RATIO
    }
}

fn split_leaf(node: &mut Node, target: PaneId, new_pane: PaneId, orientation: Orientation) -> bool {
    match node {
        Node::Pane(pane) if *pane == target => {
            *node = Node::split(
                orientation,
                DEFAULT_RATIO,
                Node::pane(target),
                Node::pane(new_pane),
            );
            true
        }
        Node::Pane(_) => false,
        Node::Split { first, second, .. } => {
            split_leaf(first, target, new_pane, orientation)
                || split_leaf(second, target, new_pane, orientation)
        }
    }
}

fn rebalance_component_containing(
    node: &mut Node,
    target: PaneId,
    orientation: Orientation,
) -> (bool, bool) {
    match node {
        Node::Pane(pane) => (*pane == target, *pane == target),
        Node::Split {
            orientation: node_orientation,
            first,
            second,
            ..
        } => {
            let (found, touches_root) = if first.contains(target) {
                rebalance_component_containing(first, target, orientation)
            } else if second.contains(target) {
                rebalance_component_containing(second, target, orientation)
            } else {
                return (false, false);
            };
            if found && touches_root && *node_orientation == orientation {
                rebalance_equal_siblings(node, orientation);
                (true, true)
            } else {
                (found, false)
            }
        }
    }
}

fn rebalance_equal_siblings(node: &mut Node, orientation: Orientation) {
    let Node::Split {
        orientation: node_orientation,
        ratio,
        first,
        second,
    } = node
    else {
        return;
    };
    if *node_orientation != orientation {
        return;
    }
    let first_units = orientation_units(first, orientation);
    let second_units = orientation_units(second, orientation);
    *ratio = first_units as f64 / (first_units + second_units) as f64;
    rebalance_equal_siblings(first, orientation);
    rebalance_equal_siblings(second, orientation);
}

fn orientation_units(node: &Node, orientation: Orientation) -> usize {
    match node {
        Node::Split {
            orientation: node_orientation,
            first,
            second,
            ..
        } if *node_orientation == orientation => {
            orientation_units(first, orientation) + orientation_units(second, orientation)
        }
        Node::Pane(_) | Node::Split { .. } => 1,
    }
}

fn remove_pane(node: Node, target: PaneId) -> (Option<Node>, Option<PaneId>, Option<Orientation>) {
    match node {
        Node::Pane(pane) if pane == target => (None, None, None),
        Node::Pane(_) => (Some(node), None, None),
        Node::Split {
            orientation,
            ratio,
            first,
            second,
        } => {
            if first.contains(target) {
                let replacement = second.first_pane();
                let (first, nested_replacement, collapsed_orientation) =
                    remove_pane(*first, target);
                match first {
                    Some(first) => (
                        Some(Node::split(orientation, ratio, first, *second)),
                        nested_replacement,
                        collapsed_orientation,
                    ),
                    None => (Some(*second), Some(replacement), Some(orientation)),
                }
            } else if second.contains(target) {
                let replacement = first.first_pane();
                let (second, nested_replacement, collapsed_orientation) =
                    remove_pane(*second, target);
                match second {
                    Some(second) => (
                        Some(Node::split(orientation, ratio, *first, second)),
                        nested_replacement,
                        collapsed_orientation,
                    ),
                    None => (Some(*first), Some(replacement), Some(orientation)),
                }
            } else {
                (
                    Some(Node::split(orientation, ratio, *first, *second)),
                    None,
                    None,
                )
            }
        }
    }
}

fn split_rect(
    rect: Rect,
    orientation: Orientation,
    ratio: f64,
    first_required: RequiredSize,
    second_required: RequiredSize,
) -> (Rect, Rect) {
    match orientation {
        Orientation::Horizontal => {
            let first_width = allocate_axis(
                rect.width,
                ratio,
                first_required.width,
                second_required.width,
            );
            (
                Rect::new(rect.x, rect.y, first_width, rect.height),
                Rect::new(
                    rect.x.saturating_add(first_width),
                    rect.y,
                    rect.width.saturating_sub(first_width),
                    rect.height,
                ),
            )
        }
        Orientation::Vertical => {
            let first_height = allocate_axis(
                rect.height,
                ratio,
                first_required.height,
                second_required.height,
            );
            (
                Rect::new(rect.x, rect.y, rect.width, first_height),
                Rect::new(
                    rect.x,
                    rect.y.saturating_add(first_height),
                    rect.width,
                    rect.height.saturating_sub(first_height),
                ),
            )
        }
    }
}

fn allocate_axis(total: u16, ratio: f64, first_minimum: u32, second_minimum: u32) -> u16 {
    if total == 0 {
        return 0;
    }

    let total_u32 = u32::from(total);
    let desired = (f64::from(total) * sanitize_ratio(ratio)).round() as u32;
    let (lower, upper) = if first_minimum.saturating_add(second_minimum) <= total_u32 {
        (first_minimum, total_u32 - second_minimum)
    } else if total > 1 {
        (1, total_u32 - 1)
    } else {
        (1, 1)
    };

    desired.clamp(lower, upper) as u16
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DirectionalScore {
    gap: u32,
    overlap: u32,
    center_offset: u32,
}

fn directional_score(
    current: Rect,
    candidate: Rect,
    direction: Direction,
) -> Option<DirectionalScore> {
    if current.width == 0 || current.height == 0 || candidate.width == 0 || candidate.height == 0 {
        return None;
    }

    let (in_direction, gap, overlap, center_offset) = match direction {
        Direction::Left => (
            candidate.right() <= u32::from(current.x),
            u32::from(current.x).saturating_sub(candidate.right()),
            overlap(
                u32::from(current.y),
                current.bottom(),
                u32::from(candidate.y),
                candidate.bottom(),
            ),
            current
                .center_y_twice()
                .abs_diff(candidate.center_y_twice()),
        ),
        Direction::Right => (
            u32::from(candidate.x) >= current.right(),
            u32::from(candidate.x).saturating_sub(current.right()),
            overlap(
                u32::from(current.y),
                current.bottom(),
                u32::from(candidate.y),
                candidate.bottom(),
            ),
            current
                .center_y_twice()
                .abs_diff(candidate.center_y_twice()),
        ),
        Direction::Up => (
            candidate.bottom() <= u32::from(current.y),
            u32::from(current.y).saturating_sub(candidate.bottom()),
            overlap(
                u32::from(current.x),
                current.right(),
                u32::from(candidate.x),
                candidate.right(),
            ),
            current
                .center_x_twice()
                .abs_diff(candidate.center_x_twice()),
        ),
        Direction::Down => (
            u32::from(candidate.y) >= current.bottom(),
            u32::from(candidate.y).saturating_sub(current.bottom()),
            overlap(
                u32::from(current.x),
                current.right(),
                u32::from(candidate.x),
                candidate.right(),
            ),
            current
                .center_x_twice()
                .abs_diff(candidate.center_x_twice()),
        ),
    };

    (in_direction && overlap > 0).then_some(DirectionalScore {
        gap,
        overlap,
        center_offset,
    })
}

fn overlap(first_start: u32, first_end: u32, second_start: u32, second_end: u32) -> u32 {
    first_end
        .min(second_end)
        .saturating_sub(first_start.max(second_start))
}

fn compare_scores(left: DirectionalScore, right: DirectionalScore) -> Ordering {
    left.gap
        .cmp(&right.gap)
        .then_with(|| right.overlap.cmp(&left.overlap))
        .then_with(|| left.center_offset.cmp(&right.center_offset))
}

fn swap_panes(node: &mut Node, first_id: PaneId, second_id: PaneId) {
    match node {
        Node::Pane(pane) if *pane == first_id => *pane = second_id,
        Node::Pane(pane) if *pane == second_id => *pane = first_id,
        Node::Pane(_) => {}
        Node::Split { first, second, .. } => {
            swap_panes(first, first_id, second_id);
            swap_panes(second, first_id, second_id);
        }
    }
}

fn replace_pane_id(node: &mut Node, old: PaneId, new: PaneId) -> bool {
    match node {
        Node::Pane(pane) if *pane == old => {
            *pane = new;
            true
        }
        Node::Pane(_) => false,
        Node::Split { first, second, .. } => {
            replace_pane_id(first, old, new) || replace_pane_id(second, old, new)
        }
    }
}

fn resize_node(
    node: &mut Node,
    focused: PaneId,
    target_orientation: Orientation,
    signed_cells: i32,
    rect: Rect,
    minimum: MinimumSize,
) -> bool {
    let Node::Split {
        orientation,
        ratio,
        first,
        second,
    } = node
    else {
        return false;
    };

    let focused_in_first = first.contains(focused);
    let focused_in_second = second.contains(focused);
    if !focused_in_first && !focused_in_second {
        return false;
    }

    let first_required = first.required_size(minimum);
    let second_required = second.required_size(minimum);
    let (first_rect, second_rect) =
        split_rect(rect, *orientation, *ratio, first_required, second_required);
    let child_resized = if focused_in_first {
        resize_node(
            first,
            focused,
            target_orientation,
            signed_cells,
            first_rect,
            minimum,
        )
    } else {
        resize_node(
            second,
            focused,
            target_orientation,
            signed_cells,
            second_rect,
            minimum,
        )
    };
    if child_resized {
        return true;
    }
    if *orientation != target_orientation {
        return false;
    }

    let total = match orientation {
        Orientation::Horizontal => rect.width,
        Orientation::Vertical => rect.height,
    };
    if total < 2 {
        return false;
    }

    let current_first = match orientation {
        Orientation::Horizontal => first_rect.width,
        Orientation::Vertical => first_rect.height,
    };
    let requested_change = if focused_in_first {
        signed_cells
    } else {
        -signed_cells
    };
    let desired_first = i64::from(current_first) + i64::from(requested_change);
    let first_minimum = match orientation {
        Orientation::Horizontal => first_required.width,
        Orientation::Vertical => first_required.height,
    };
    let second_minimum = match orientation {
        Orientation::Horizontal => second_required.width,
        Orientation::Vertical => second_required.height,
    };
    let total_u32 = u32::from(total);
    let (lower, upper) = if first_minimum.saturating_add(second_minimum) <= total_u32 {
        (first_minimum, total_u32 - second_minimum)
    } else {
        (1, total_u32 - 1)
    };
    let next_first = desired_first.clamp(i64::from(lower), i64::from(upper)) as u32;
    if next_first == u32::from(current_first) {
        return false;
    }

    *ratio = f64::from(next_first) / f64::from(total);
    true
}
