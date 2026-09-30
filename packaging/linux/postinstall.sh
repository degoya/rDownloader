#!/bin/sh
# Says how to start the service; touches nothing. No script of the deb or rpm package reads,
# moves or deletes a user's data (RD-180-05): removing the package leaves
# ~/.local/share/rdownloader as it is.
echo "rDownloader: start it per user with 'systemctl --user daemon-reload && systemctl --user enable --now rdownloader',"
echo "then open http://127.0.0.1:8710. After an upgrade: 'systemctl --user restart rdownloader'."
exit 0
