#version 330 core

// Fullscreen triangle via gl_VertexID -- no VBO needed.
// Fragment shader uses gl_FragCoord.xy directly, no varyings required.
void main() {
    float x = float((gl_VertexID & 1) * 4 - 1);
    float y = float((gl_VertexID >> 1) * 4 - 1);
    gl_Position = vec4(x, y, 0.0, 1.0);
}
