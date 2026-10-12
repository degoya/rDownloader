-- Whether a category is offered in the pickers that assign one by hand (RD-1260-03): the
-- LinkGrabber and the Downloads. Off keeps it for rules, subscriptions, hotfolders, the *arr
-- interfaces, REST and MCP. The default category always stays selectable.
ALTER TABLE categories ADD COLUMN selectable INTEGER NOT NULL DEFAULT 1;
