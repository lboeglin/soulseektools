import json
import sys
import urllib.parse
import urllib.request

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

QUERIES = [
    ("ALICESOFT", "Rebirth the edge"),
    ("beatMARIO", "Night of Knights"),
    ("COOL&CREATE", "Reisen Occultia"),
    ("DECO*27", "Yowamushi Montblanc"),
    ("Garrett Williamson", "The Girl Who Glitched"),
    ("Iyowa", "Heat Abnormal"),
    ("Mikito P", "Roki"),
    ("Reol", "LUVORATORRRRRY!"),
    ("ぐちり", "Schadenfreude"),
    ("ZUN", "Heian Alien"),
    ("香椎モイミ", "Cat Loving"),
]

for artist, title in QUERIES:
    query = urllib.parse.quote(f"{artist} {title}")
    url = f"https://itunes.apple.com/search?term={query}&media=music&limit=1"
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        data = json.loads(resp.read())
    if data.get("results"):
        r = data["results"][0]
        print(f"query='{artist} - {title}' -> matched artistName='{r.get('artistName')}' track='{r.get('trackName')}'")
    else:
        print(f"query='{artist} - {title}' -> NO RESULT")
