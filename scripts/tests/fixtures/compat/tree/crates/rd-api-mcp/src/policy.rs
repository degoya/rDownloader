pub const TOOL_POLICY: &[ToolPolicy] = &[
    tool("get_item", "/api/v1/items/{id}", Method::GET),
    tool(
        "list_items",
        "/api/v1/items",
        Method::GET,
    ),
];
