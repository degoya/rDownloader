-- RD-150-09: named parts a sign-in keeps beside its token.
--
-- Real-Debrid's open-source device flow hands every person a client id and a client secret of
-- their own, and every renewal needs both beside the refresh material. One vault reference per
-- sign-in could only hold them joined, and a joined value cannot be split again once the host has
-- percent-encoded it into a request. So each part is a row of its own here: a name the provider
-- declares as a flow-filled slot, and a vault reference like `auth_flows.access_ref`.
--
-- Same rules as the flow's other references: never returned through the API, the value never in
-- this file, and gone with the account.
CREATE TABLE auth_flow_parts (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    secret_ref TEXT NOT NULL,
    PRIMARY KEY (account_id, name)
);
