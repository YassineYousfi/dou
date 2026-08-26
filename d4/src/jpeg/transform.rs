// (swap axes, flip source x, flip source y), indexed by the CLI operation.
const GEOMETRY: [(bool, bool, bool); 8] = [
    (false, false, false), // identity
    (true, false, true),   // rotate 90
    (false, true, true),   // rotate 180
    (true, true, false),   // rotate 270
    (false, true, false),  // flip horizontal
    (false, false, true),  // flip vertical
    (true, false, false),  // transpose
    (true, true, true),    // transverse
];

#[derive(Clone, Copy)]
pub(super) struct Transform(u8);

impl Transform {
    pub fn from_index(index: u8) -> Option<Self> {
        (index < 8).then_some(Self(index))
    }

    pub fn filename(self) -> String {
        format!("{}.jpg", self.0)
    }

    pub fn swaps_axes(self) -> bool {
        self.geometry().0
    }

    // Map a destination position back to the source. This works for both the
    // MCU grid and the block grid inside a component's MCU.
    pub fn source_position(
        self,
        destination_x: usize,
        destination_y: usize,
        source_width: usize,
        source_height: usize,
    ) -> (usize, usize) {
        let (swap, flip_x, flip_y) = self.geometry();
        let (mut source_x, mut source_y) = if swap {
            (destination_y, destination_x)
        } else {
            (destination_x, destination_y)
        };
        if flip_x {
            source_x = source_width - 1 - source_x;
        }
        if flip_y {
            source_y = source_height - 1 - source_y;
        }
        (source_x, source_y)
    }

    pub fn apply_block(self, mut source: impl FnMut(usize, usize) -> i16) -> [i16; 64] {
        let (swap, flip_x, flip_y) = self.geometry();
        let mut output = [0_i16; 64];
        for v in 0..8 {
            for u in 0..8 {
                let (source_u, source_v) = if swap { (v, u) } else { (u, v) };
                let negative = (flip_x && source_u & 1 != 0) ^ (flip_y && source_v & 1 != 0);
                let sign = if negative { -1 } else { 1 };
                output[v * 8 + u] = source(source_u, source_v) * sign;
            }
        }
        output
    }

    fn geometry(self) -> (bool, bool, bool) {
        GEOMETRY[usize::from(self.0)]
    }
}

pub(super) fn transpose_frequency_table(source: &[u16; 64]) -> [u16; 64] {
    let mut output = [0; 64];
    for row in 0..8 {
        for column in 0..8 {
            output[row * 8 + column] = source[column * 8 + row];
        }
    }
    output
}
