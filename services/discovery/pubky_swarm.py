"""qBittorrent search plugin for a local Torky discovery service."""

import json
import urllib.parse
import urllib.request

from novaprinter import prettyPrinter


class pubky_swarm:
    """Search the service's opt-in, non-authoritative validated release cache."""

    url = "http://127.0.0.1:7780"
    name = "Torky"
    supported_categories = {"all": "all"}

    def search(self, what, cat="all"):
        del cat
        decoded = urllib.parse.unquote_plus(what)
        query = urllib.parse.urlencode({"q": decoded[:256], "limit": 100})
        endpoint = f"{self.url}/v1/search?{query}"
        request = urllib.request.Request(
            endpoint,
            headers={"Accept": "application/json", "User-Agent": "qBittorrent-Torky/1"},
        )
        with urllib.request.urlopen(request, timeout=15) as response:
            results = json.load(response)
        for result in results:
            release = result["release"]
            torrent = release["torrent"]
            magnet_parameters = [
                ("xt", f"urn:btih:{torrent['info_hash']}"),
                ("dn", release["title"]),
            ]
            magnet_parameters.extend(("tr", tracker) for tracker in torrent.get("trackers", []))
            magnet = "magnet:?" + urllib.parse.urlencode(magnet_parameters)
            prettyPrinter(
                {
                    "name": release["title"],
                    "link": magnet,
                    "size": torrent["size"],
                    "seeds": -1,
                    "leech": -1,
                    "engine_url": self.url,
                    "desc_link": result["details"],
                }
            )
