#!/usr/bin/env python3
"""Generate focus grid examples for the Grid component specification.

Usage:
  python3 focus_grids.py <example_name>

Examples:
  python3 focus_grids.py 2x2                 # 2x2 grid, 4 focus states (A/B/C/D), no merges
  python3 focus_grids.py 2x3_merged          # 2x3 grid, B+E merged, focus A/BE/C (row 0)
  python3 focus_grids.py 2x3_merged_from_d   # 2x3 grid, B+E merged, focus D/BE/F (row 1)
"""

import sys

CELL_W = 9
CELL_H = 5
FL = "▛"; FT = "▀"; FR = "▜"; FBLL = "▙"; FB = "▄"; FBR = "▟"; FLL = "▌"; FRR = "▐"
IBG = "░"
CORRECT = {
    "▛": 0x259B, "▀": 0x2580, "▜": 0x259C,
    "▙": 0x2599, "▄": 0x2584, "▟": 0x259F,
    "▌": 0x258C, "▐": 0x2590,
    "▓": 0x2593, "░": 0x2591, "▒": 0x2592,
    "╳": 0x2573, "█": 0x2588,
}

def focused_normal(label):
    inside = CELL_W - 2
    bar = inside - 2
    pad = (inside - 1) // 2
    return [
        f"{FL} {FT * bar} {FR}",
        f" {IBG * inside} ",
        f"{FLL}{IBG * pad}{label}{IBG * (inside - 1 - pad)}{FRR}",
        f" {IBG * inside} ",
        f"{FBLL} {FB * bar} {FBR}",
    ]

def normal_cell(label, bg):
    pad = (CELL_W - 1) // 2
    return [
        bg * CELL_W,
        bg * CELL_W,
        bg * pad + label + bg * (CELL_W - 1 - pad),
        bg * CELL_W,
        bg * CELL_W,
    ]

def focused_merged(label, height):
    inside = CELL_W - 2
    bar = inside - 2
    mid = height // 2
    pad = (inside - len(label)) // 2
    lines = []
    for row in range(height):
        if row == 0:
            lines.append(f"{FL} {FT * bar} {FR}")
        elif row == height - 1:
            lines.append(f"{FBLL} {FB * bar} {FBR}")
        elif row == 1 or row == height - 2:
            lines.append(f" {IBG * inside} ")
        elif row == mid:
            lines.append(f"{FLL}{IBG * pad}{label}{IBG * (inside - len(label) - pad)}{FRR}")
        else:
            lines.append(f"{FLL}{IBG * inside}{FRR}")
    return lines

def normal_merged(label, bg, height):
    mid = height // 2
    pad = (CELL_W - len(label)) // 2
    lines = []
    for row in range(height):
        if row == mid:
            lines.append(bg * pad + label + bg * (CELL_W - len(label) - pad))
        else:
            lines.append(bg * CELL_W)
    return lines

def verify(lines, total_w):
    for i, line in enumerate(lines):
        assert len(line) == total_w, f"Line {i}: {len(line)} != {total_w}"
        for j, ch in enumerate(line):
            if ch in CORRECT:
                assert ord(ch) == CORRECT[ch], f"Line {i} pos {j}: U+{ord(ch):04X} should be U+{CORRECT[ch]:04X}"

def example_2x2():
    COLS = 2; ROWS = 2
    GRID_W = COLS * CELL_W
    SEP = "  "; NUM_GRIDS = 4
    TOTAL_W = NUM_GRIDS * GRID_W + (NUM_GRIDS - 1) * len(SEP)
    LABELS = ["A", "B", "C", "D"]
    BGS = ["▓", "░", "▒", "╳"]

    def generate_grid(focus_idx):
        grid_rows = []
        for r in range(ROWS):
            cell_rows = []
            for c in range(COLS):
                idx = r * COLS + c
                if idx == focus_idx:
                    cell_rows.append(focused_normal(LABELS[idx]))
                else:
                    cell_rows.append(normal_cell(LABELS[idx], BGS[idx]))
            for row in range(CELL_H):
                combined = "".join(cell_rows[c][row] for c in range(COLS))
                assert len(combined) == GRID_W
                grid_rows.append(combined)
        return grid_rows

    grids = [generate_grid(i) for i in range(NUM_GRIDS)]
    lines = []
    for row in range(ROWS * CELL_H):
        parts = []
        for g in range(NUM_GRIDS):
            if g > 0: parts.append(SEP)
            parts.append(grids[g][row])
        lines.append("".join(parts))
    verify(lines, TOTAL_W)
    return lines

def example_2x3_merged(focus_start="A"):
    COLS = 3; ROWS = 2; MERGED_H = 10
    GRID_W = COLS * CELL_W
    SEP = "  "; NUM_GRIDS = 3
    TOTAL_W = NUM_GRIDS * GRID_W + (NUM_GRIDS - 1) * len(SEP)

    if focus_start == "A":
        focus_indices = [0, 1, 2]
        header = "  Start                → BE                → C"
    else:
        focus_indices = [0, 1, 2]
        header = "  Start                → BE                → F"

    def generate_grid(focus_col):
        if focus_start == "A":
            col0_top = focused_normal("A") if focus_col == 0 else normal_cell("A", "▓")
            col0_bot = normal_cell("D", "╳")
            col2_top = focused_normal("C") if focus_col == 2 else normal_cell("C", "▒")
            col2_bot = normal_cell("F", "▓")
        else:
            col0_top = normal_cell("A", "▓")
            col0_bot = focused_normal("D") if focus_col == 0 else normal_cell("D", "╳")
            col2_top = normal_cell("C", "▒")
            col2_bot = focused_normal("F") if focus_col == 2 else normal_cell("F", "▓")
        col1 = focused_merged("BE", MERGED_H) if focus_col == 1 else normal_merged("BE", "░", MERGED_H)
        grid_rows = []
        for row in range(MERGED_H):
            if row < 5: c0 = col0_top[row]; c2 = col2_top[row]
            else: c0 = col0_bot[row - 5]; c2 = col2_bot[row - 5]
            c1 = col1[row]
            combined = c0 + c1 + c2
            assert len(combined) == GRID_W
            grid_rows.append(combined)
        return grid_rows

    grids = [generate_grid(g) for g in focus_indices]
    lines = []
    lines.append(header)
    lines.append("")
    for row in range(MERGED_H):
        parts = []
        for g in range(NUM_GRIDS):
            if g > 0: parts.append(SEP)
            parts.append(grids[g][row])
        lines.append("".join(parts))
    verify(lines[2:], TOTAL_W)
    return lines

EXAMPLES = {
    "2x2": lambda: example_2x2(),
    "2x3_merged": lambda: example_2x3_merged("A"),
    "2x3_merged_from_d": lambda: example_2x3_merged("D"),
}

if __name__ == "__main__":
    name = sys.argv[1] if len(sys.argv) > 1 else "2x2"
    if name not in EXAMPLES:
        print(f"Unknown example: {name}. Available: {', '.join(EXAMPLES)}")
        sys.exit(1)
    lines = EXAMPLES[name]()
    for line in lines:
        print(line)
    print(f"\n--- {name}: {len(lines)} lines, verified OK ---")
