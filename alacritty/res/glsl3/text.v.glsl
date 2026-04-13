// Cell properties.
layout(location = 0) in vec2 gridCoords;

// Glyph properties.
layout(location = 1) in vec4 glyph;

// uv mapping.
layout(location = 2) in vec4 uv;

// Text foreground rgb packed together with cell flags. textColor.a
// are the bitflags; consult RenderingGlyphFlags in renderer/mod.rs
// for the possible values.
layout(location = 3) in vec4 textColor;

// Background color.
layout(location = 4) in vec4 backgroundColor;

out vec2 TexCoords;
flat out vec4 fg;
flat out vec4 bg;

// Terminal properties
uniform vec2 cellDim;
uniform vec4 projection;

uniform int renderingPass;

#define COLORED 1
#define WIDE_CHAR 2

void main() {
    vec2 projectionOffset = projection.xy;
    vec2 projectionScale = projection.zw;

    // Compute vertex corner position
    vec2 position;
    position.x = (gl_VertexID == 0 || gl_VertexID == 1) ? 1. : 0.;
    position.y = (gl_VertexID == 0 || gl_VertexID == 3) ? 0. : 1.;

    // Position of cell from top-left
    vec2 cellPosition = cellDim * gridCoords;

    fg = vec4(textColor.rgb / 255.0, textColor.a);
    bg = backgroundColor / 255.0;

    // Extract cell span from bits 2-4 of the flags byte (1-8 cells).
    // Round to avoid float truncation drift (e.g. int(4.999) -> 4).
    int flags = int(fg.a + 0.5);
    int cellSpan = max(1, (flags >> 2) & 0x7);
    float occupiedCells = float(cellSpan);

    // For backward compat: if no span encoded but WIDE_CHAR flag set, use 2.
    if (cellSpan <= 1 && (flags & WIDE_CHAR) != 0) {
        occupiedCells = 2.0;
    }

    // Strip span and WIDE_CHAR bits, keeping only COLORED.
    fg.a = float(flags & COLORED);

    if (renderingPass == 0) {
        vec2 backgroundDim = cellDim;
        backgroundDim.x *= occupiedCells;

        vec2 finalPosition = cellPosition + backgroundDim * position;
        gl_Position =
            vec4(projectionOffset + projectionScale * finalPosition, 0.0, 1.0);

        TexCoords = vec2(0, 0);
    } else {
        vec2 glyphSize = glyph.zw;
        vec2 glyphOffset = glyph.xy;
        glyphOffset.y = cellDim.y - glyphOffset.y;

        vec2 finalPosition = cellPosition + glyphSize * position + glyphOffset;
        gl_Position =
            vec4(projectionOffset + projectionScale * finalPosition, 0.0, 1.0);

        vec2 uvOffset = uv.xy;
        vec2 uvSize = uv.zw;
        TexCoords = uvOffset + position * uvSize;
    }
}
