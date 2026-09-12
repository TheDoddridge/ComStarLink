use axum::{
    extract::{Path, Query, State},
    response::Html,
    routing::{delete, get, post},
    Form, Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

// --- DATA MODELS ---

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Comment {
    author: String,
    text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Contract {
    id: u64,
    host: String,
    host_force: String,
    host_assets: String,

    challenger: Option<String>,
    challenger_force: Option<String>,
    challenger_assets: Option<String>,

    #[serde(default)]
    mission_type: String,
    bv2: String,
    era: String,
    map: String,
    intel_level: String,
    match_time: String,
    #[serde(default)]
    expires_at: i64,
    force_comp: String,
    ruleset: String,
    optional_rules: Vec<String>,
    status: String,
    comments: Vec<Comment>,
}
#[derive(Clone)]
struct AppState {
    db: sled::Db,
}

#[derive(Deserialize)]
struct RulesetQuery {
    ruleset: String,
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
}

#[derive(Deserialize)]
struct AcceptForm {
    challenger_force: String,
    challenger_assets: String,
}

#[derive(Deserialize)]
struct CommentForm {
    text: String,
}

fn format_force(force_string: &str, intel_level: &str) -> String {
    if force_string.is_empty() {
        return "None".to_string();
    }

    if intel_level.contains("Blackout") {
        return "<span style='color: #ff7b72;'>[ CLASSIFIED ]</span>".to_string();
    }

    if intel_level.contains("Intercept") {
        let mut counts: HashMap<String, u32> = HashMap::new();

        for unit in force_string.split(',') {
            if let Some(start) = unit.find('[') {
                if let Some(end) = unit.find(']') {
                    let class = &unit[start + 1..end];
                    *counts.entry(class.to_string()).or_insert(0) += 1;
                }
            }
        }

        let mut summary = Vec::new();
        for (class, count) in counts {
            summary.push(format!("{}x {}", count, class));
        }
        summary.sort();

        if summary.is_empty() {
            return "<span style='color: #e3b341;'>[ ENCRYPTED / UNKNOWN ]</span>".to_string();
        }
        return summary.join(", ");
    }

    force_string.replace(",", "<br>• ")
}

fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn cleanup_expired_contracts(db: &sled::Db) {
    let now = current_unix_timestamp();

    for item in db.iter() {
        let Ok((key, value)) = item else {
            continue;
        };

        let Ok(contract) = serde_json::from_slice::<Contract>(&value) else {
            continue;
        };

        // expires_at == 0 means this is an older contract created before
        // expiration tracking was added. Leave it alone rather than risking
        // deleting an existing contract that cannot be safely dated.
        if contract.expires_at > 0 && now >= contract.expires_at {
            let _ = db.remove(key);
        }
    }
}

// --- HTML RENDERING ---

fn render_contract(c: &Contract, current_user: &str) -> String {
    let host_force_display = format_force(&c.host_force, &c.intel_level);
    let bv2_display = if c.bv2.is_empty() {
        "Open".to_string()
    } else {
        c.bv2.clone()
    };
    let host_assets_display = if c.host_assets.is_empty() {
        "None".to_string()
    } else {
        c.host_assets.clone()
    };

    // 1. Action Area (Accept / Transfer / Withdraw logic)
    let action_html = if c.status == "Open" {
        if current_user == c.host {
            r##"<div style="margin-top: 15px; padding-top: 15px; border-top: 1px dashed #39d353;">
                <em>[ Awaiting Challenger Signatures... ]</em>
            </div>"##
                .to_string()
        } else {
            format!(
                r##"<form hx-post="/contract/{id}/accept" hx-target="#contract-{id}" hx-swap="outerHTML" style="margin-top: 15px; padding-top: 15px; border-top: 1px dashed #39d353;">
                    <strong style="color: #ff7b72;">:: ACCEPT CONTRACT ::</strong>
                    
                    <div style="margin-top: 10px;">
                        <label>Challenger Force Roster:</label>
                        <div style="display:flex; gap:10px; margin-top:5px;">
                            <input type="text" id="challenger-unit-search-{id}" list="unit-datalist" placeholder="Search Unit..." style="margin-bottom:0;">
                            <button type="button" onclick="addUnit('challenger', '{id}')" style="margin-bottom:0; width:auto;">ADD</button>
                        </div>
                        <ul id="challenger-roster-list-{id}" style="list-style: none; padding: 0; margin: 10px 0; font-size: 0.9em;"></ul>
                        <input type="hidden" name="challenger_force" id="challenger_force_input_{id}" value="">
                    </div>
                    
                    <input type="text" name="challenger_assets" placeholder="Assets (e.g., Dice & Tukayyid maps)" style="margin-top: 10px;">
                    <button type="submit" style="background: #ff7b72; color: #0d1117; border-color: #ff7b72;">LOCK MATCH</button>
                </form>"##,
                id = c.id
            )
        }
    } else {
        // Match is Locked. Generate context-sensitive buttons.
        let mut action_buttons = String::new();

        if current_user == c.host {
            // The Host can transfer command if they need to bail
            action_buttons.push_str(&format!(
                r##"<button hx-post="/contract/{id}/transfer" hx-target="#contract-{id}" hx-swap="outerHTML" style="background: transparent; color: #58a6ff; border: 1px solid #58a6ff; width: auto; margin-bottom: 0; padding: 5px 10px;">
                    TRANSFER COMMAND TO CHALLENGER
                </button>"##,
                id = c.id
            ));
        } else if Some(current_user.to_string()) == c.challenger {
            // The Challenger can withdraw
            action_buttons.push_str(&format!(
                r##"<button hx-post="/contract/{id}/withdraw" hx-target="#contract-{id}" hx-swap="outerHTML" style="background: #ff7b72; color: #0d1117; border-color: #ff7b72; width: auto; margin-bottom: 0; padding: 5px 10px;">
                    WITHDRAW
                </button>"##,
                id = c.id
            ));
        }

        let challenger_force_display =
            format_force(c.challenger_force.as_deref().unwrap_or(""), &c.intel_level);
        let challenger_assets = c.challenger_assets.as_deref().unwrap_or("").trim();
        let challenger_assets_display = if challenger_assets.is_empty() {
            "None"
        } else {
            challenger_assets
        };

        let challenger_info = format!(
            r##"<div style="margin-top: 10px; padding: 10px; background: #0d1117; border: 1px solid #ff7b72;">
                <strong>Challenger:</strong> {challenger}<br>
                <strong>Force:</strong> <br>• {force}<br>
                <strong style="margin-top: 5px; display: inline-block;">Assets:</strong> {assets}
            </div>"##,
            challenger = c.challenger.as_deref().unwrap_or("Unknown"),
            force = challenger_force_display,
            assets = challenger_assets_display,
        );

        format!(
            r##"<div style="margin-top: 15px; padding-top: 15px; border-top: 1px dashed #39d353;">
                <div style="display: flex; gap: 10px; align-items: center; margin-bottom: 10px;">
                    <span class="accepted">[ MATCH LOCKED - AWAITING DEPLOYMENT ]</span>
                    {action_buttons}
                </div>
                {challenger_info}
            </div>"##,
            action_buttons = action_buttons,
            challenger_info = challenger_info
        )
    };

    // 2. Comments Area
    let mut comments_html = String::new();
    for comment in &c.comments {
        comments_html.push_str(&format!(
            r##"<div style="margin-top: 5px; font-size: 0.9em;"><strong>{author}:</strong> {text}</div>"##,
            author = comment.author,
            text = comment.text
        ));
    }

    if comments_html.is_empty() {
        comments_html = "<em>No encrypted comms.</em>".to_string();
    }

    let comments_section = format!(
        r##"<div style="margin-top: 15px; padding-top: 15px; border-top: 1px dashed #39d353;">
            <strong style="color: #58a6ff;">:: COMMS CHANNEL ::</strong>
            <div style="margin-bottom: 10px; max-height: 150px; overflow-y: auto;">{comments_html}</div>
            <form hx-post="/contract/{id}/comment" hx-target="#contract-{id}" hx-swap="outerHTML" style="display: flex; gap: 10px;">
                <input type="text" name="text" placeholder="Transmit message..." required style="margin-bottom: 0;">
                <button type="submit" style="margin-bottom: 0; width: auto; padding: 0 20px;">SEND</button>
            </form>
        </div>"##,
        id = c.id,
        comments_html = comments_html
    );

    // 3. Optional Rules and Final Assembly
    let optional_rules_html = if c.optional_rules.is_empty() {
        "None".to_string()
    } else {
        c.optional_rules.join(", ")
    };

    let cancel_button = if current_user == c.host {
        format!(
            r##"<button hx-delete="/contract/{id}" hx-confirm="Are you sure you want to scrub this mission? This cannot be undone." hx-target="#contract-{id}" hx-swap="outerHTML" style="background: transparent; color: #ff7b72; border: 1px solid #ff7b72; padding: 5px 10px; margin-top: 15px; width: 100%;">
                SCRUB MISSION (CANCEL CONTRACT)
            </button>"##,
            id = c.id
        )
    } else {
        String::new()
    };

    format!(
        r##"<div class="card" id="contract-{id}">
            <div style="display: flex; justify-content: space-between; border-bottom: 1px solid #39d353; margin-bottom: 10px; padding-bottom: 5px;">
                <strong>Contract #{id} - Host: {host}</strong>
                <span style="color: #e3b341;" class="countdown" data-time="{match_time}">CALCULATING JUMP...</span>
            </div>
            
            <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 10px;">
                <div>
                    <strong>Parameters:</strong> {mission_type} | {bv2} BV | {force_comp} | {era}<br>
                    <strong>Intel Level:</strong> {intel_level}<br>
                    <strong>Ruleset:</strong> {ruleset}<br>
                    <strong>Optional Rules:</strong> {optional_rules_html}
                </div>
                <div style="border-left: 1px dashed #39d353; padding-left: 10px;">
                    <strong>Host Force:</strong><br>• {host_force_display}<br>
                    <strong style="margin-top: 5px; display: inline-block;">Host Assets:</strong> {host_assets}
                </div>
            </div>
            
            {action_html}
            {comments_section}
            {cancel_button}
        </div>"##,
        id = c.id,
        host = c.host,
        match_time = c.match_time,
        mission_type = c.mission_type,
        bv2 = bv2_display,
        force_comp = c.force_comp,
        era = c.era,
        intel_level = c.intel_level,
        ruleset = c.ruleset,
        optional_rules_html = optional_rules_html,
        host_force_display = host_force_display,
        host_assets = host_assets_display,
        action_html = action_html,
        comments_section = comments_section,
        cancel_button = cancel_button
    )
}

fn render_board(contracts: &[Contract], current_user: &str) -> String {
    let mut rows = String::new();
    for c in contracts.iter().rev() {
        rows.push_str(&render_contract(c, current_user));
    }

    let header = format!(
        r##"
    <!DOCTYPE html>
    <html lang="en">
    <head>
        <meta charset="UTF-8">
        <title>LGS ComStar Terminal</title>
        <script src="https://unpkg.com/htmx.org@1.9.10"></script>
        <style>
            body {{ background-color: #0d1117; color: #39d353; font-family: monospace; max-width: 900px; margin: 0 auto; padding: 20px; }}
            .card {{ border: 1px solid #39d353; padding: 15px; margin-bottom: 20px; background: #161b22; }}
            input, select, button {{ background: #0d1117; border: 1px solid #39d353; color: #39d353; padding: 8px; font-family: monospace; margin-bottom: 10px; width: 100%; box-sizing: border-box; }}
            
            input[type="datetime-local"]::-webkit-calendar-picker-indicator {{
                cursor: pointer;
                filter: invert(72%) sepia(35%) saturate(1005%) hue-rotate(75deg) brightness(99%) contrast(90%);
            }}

            button {{ cursor: pointer; font-weight: bold; width: auto; padding: 10px 20px; transition: 0.2s; }}
            button:hover {{ background: #39d353; color: #0d1117; }}
            .accepted {{ color: #ff7b72; border-color: #ff7b72; font-weight: bold; }}
            .form-grid {{ display: grid; grid-template-columns: 1fr 1fr; gap: 15px; }}
            .checkbox-group {{ margin: 10px 0; border: 1px dashed #39d353; padding: 10px; }}
            .checkbox-group label {{ display: block; cursor: pointer; margin-bottom: 5px; }}
            .checkbox-group input {{ width: auto; margin-right: 10px; }}
            .top-bar {{ display: flex; justify-content: space-between; align-items: center; border-bottom: 2px solid #39d353; margin-bottom: 20px; padding-bottom: 10px; }}
        </style>
    </head>
    <body>
        <div class="top-bar">
            <h2>:: MERCENARY REVIEW BOARD ::</h2>
            <span>Logged in as: <strong style="color: #58a6ff;">{current_user}</strong></span>
        </div>
<datalist id="unit-datalist">
    <option value="Ares [superheavy]">
    <option value="Omega [superheavy]">
    <option value="Orca [superheavy]">
    <option value="Poseidon [superheavy]">
    <option value="&#x27;Harvester Tripod&#x27; [assault]">
    <option value="Akuma [assault]">
    <option value="Albatross [assault]">
    <option value="Alpha Wolf [assault]">
    <option value="Amarok [assault]">
    <option value="Annihilator [assault]">
    <option value="Archangel [assault]">
    <option value="Atlas [assault]">
    <option value="Atlas II [assault]">
    <option value="Atlas III [assault]">
    <option value="Awesome [assault]">
    <option value="Banshee [assault]">
    <option value="Banzai [assault]">
    <option value="BattleMaster [assault]">
    <option value="Behemoth [assault]">
    <option value="Berserker [assault]">
    <option value="Black Watch [assault]">
    <option value="Blood Kite [assault]">
    <option value="Bruin [assault]">
    <option value="Bull Shark [assault]">
    <option value="Canis [assault]">
    <option value="Cerberus [assault]">
    <option value="Charger [assault]">
    <option value="Colossus [assault]">
    <option value="Corsair [assault]">
    <option value="Crockett [assault]">
    <option value="Crucible [assault]">
    <option value="Cudgel [assault]">
    <option value="Cyclops [assault]">
    <option value="Cygnus [assault]">
    <option value="Daboku [assault]">
    <option value="Daishi [assault]">
    <option value="Deimos [assault]">
    <option value="Devastator [assault]">
    <option value="Diomede [assault]">
    <option value="Doloire [assault]">
    <option value="Emperor [assault]">
    <option value="Epimetheus [assault]">
    <option value="Fafnir [assault]">
    <option value="Gladiator [assault]">
    <option value="Gladiator-B [assault]">
    <option value="Goliath [assault]">
    <option value="Grand Crusader [assault]">
    <option value="Grand Crusader II [assault]">
    <option value="Grand Titan [assault]">
    <option value="Great Turtle [assault]">
    <option value="Grotesque (Mongrel) [assault]">
    <option value="Gunslinger [assault]">
    <option value="Hatamoto-Chi [assault]">
    <option value="Hatamoto-Godai [assault]">
    <option value="Hatamoto-Hi [assault]">
    <option value="Hatamoto-Kaeru [assault]">
    <option value="Hatamoto-Kaze [assault]">
    <option value="Hatamoto-Ku [assault]">
    <option value="Hatamoto-Mizo [assault]">
    <option value="Hatamoto-Suna [assault]">
    <option value="Hauptmann [assault]">
    <option value="HawkWolf [assault]">
    <option value="Hellstar [assault]">
    <option value="Highlander [assault]">
    <option value="Highlander IIC [assault]">
    <option value="Imp [assault]">
    <option value="Iron Cheetah [assault]">
    <option value="Jade Phoenix [assault]">
    <option value="Juggernaut [assault]">
    <option value="Juliano [assault]">
    <option value="Jupiter [assault]">
    <option value="Katana (Crockett) [assault]">
    <option value="King Crab [assault]">
    <option value="Kingfisher [assault]">
    <option value="Kiso [assault]">
    <option value="Kodiak [assault]">
    <option value="Kodiak II [assault]">
    <option value="Kraken [assault]">
    <option value="Kraken-XR [assault]">
    <option value="Legacy [assault]">
    <option value="Lich [assault]">
    <option value="Longbow [assault]">
    <option value="Lu Wei Bing [assault]">
    <option value="Mackie [assault]">
    <option value="Mad Cat Mk II [assault]">
    <option value="Malice [assault]">
    <option value="Man O&#x27; War [assault]">
    <option value="Marauder II [assault]">
    <option value="Marauder IIC [assault]">
    <option value="Masakari [assault]">
    <option value="Mastodon [assault]">
    <option value="Mauler [assault]">
    <option value="Naga [assault]">
    <option value="Naga II [assault]">
    <option value="Naginata [assault]">
    <option value="Neanderthal [assault]">
    <option value="Night Wolf [assault]">
    <option value="Nightstar [assault]">
    <option value="O-Bakemono [assault]">
    <option value="Omen [assault]">
    <option value="Onager [assault]">
    <option value="Orochi [assault]">
    <option value="Osteon [assault]">
    <option value="Peacekeeper [assault]">
    <option value="Pendragon [assault]">
    <option value="Phoenix Hawk IIC [assault]">
    <option value="Pillager [assault]">
    <option value="Pulverizer [assault]">
    <option value="Rampage [assault]">
    <option value="Regent [assault]">
    <option value="Rifleman II [assault]">
    <option value="Rifleman III [assault]">
    <option value="Sagittaire [assault]">
    <option value="Salamander [assault]">
    <option value="Sasquatch [assault]">
    <option value="Savage Coyote [assault]">
    <option value="Scavenger [assault]">
    <option value="Schwerer Gustav [assault]">
    <option value="Scylla [assault]">
    <option value="Seraph [assault]">
    <option value="Shogun [assault]">
    <option value="Shrike [assault]">
    <option value="Sirocco [assault]">
    <option value="Spartan [assault]">
    <option value="St. Florian [assault]">
    <option value="Stalker [assault]">
    <option value="Stalker II [assault]">
    <option value="Star Adder [assault]">
    <option value="Star Crusader [assault]">
    <option value="Storm Giant [assault]">
    <option value="Striker [assault]">
    <option value="Sunder [assault]">
    <option value="Supernova [assault]">
    <option value="Tai-sho [assault]">
    <option value="Templar [assault]">
    <option value="Templar III [assault]">
    <option value="Tenshi [assault]">
    <option value="Thug [assault]">
    <option value="Thunder Hawk [assault]">
    <option value="Thunder Stallion [assault]">
    <option value="Titan [assault]">
    <option value="Titan II [assault]">
    <option value="Tomahawk [assault]">
    <option value="Tomahawk II [assault]">
    <option value="Trebaruna [assault]">
    <option value="Turkina [assault]">
    <option value="Vampyr [assault]">
    <option value="Vanquisher [assault]">
    <option value="Victor [assault]">
    <option value="Viking [assault]">
    <option value="Viking IIC [assault]">
    <option value="Wakazashi [assault]">
    <option value="Warhammer IIC [assault]">
    <option value="Warlord [assault]">
    <option value="Xanthos [assault]">
    <option value="Ymir [assault]">
    <option value="Yu Huang [assault]">
    <option value="Zeus [assault]">
    <option value="Zeus-X [assault]">
    <option value="&#x27;Battle Tripod&#x27; [heavy]">
    <option value="Anvil [heavy]">
    <option value="Anzu [heavy]">
    <option value="Arcas [heavy]">
    <option value="Archer [heavy]">
    <option value="Argus [heavy]">
    <option value="Avatar [heavy]">
    <option value="Axman [heavy]">
    <option value="Balius [heavy]">
    <option value="Bandersnatch [heavy]">
    <option value="Barghest [heavy]">
    <option value="BattleAxe [heavy]">
    <option value="Bellerophon [heavy]">
    <option value="Black Hawk-KU [heavy]">
    <option value="Black Knight [heavy]">
    <option value="Blood Reaper [heavy]">
    <option value="Bombardier [heavy]">
    <option value="Boreas [heavy]">
    <option value="Bowman [heavy]">
    <option value="Brahma [heavy]">
    <option value="Burrock [heavy]">
    <option value="Burrower [heavy]">
    <option value="Caesar [heavy]">
    <option value="Carronade [heavy]">
    <option value="Cataphract [heavy]">
    <option value="Catapult [heavy]">
    <option value="Catapult II [heavy]">
    <option value="Cauldron-Born [heavy]">
    <option value="Cave Lion [heavy]">
    <option value="Cestus [heavy]">
    <option value="Champion [heavy]">
    <option value="Crossbow [heavy]">
    <option value="Crusader [heavy]">
    <option value="Daikyu [heavy]">
    <option value="Deep Lord [heavy]">
    <option value="Defiance [heavy]">
    <option value="Deva [heavy]">
    <option value="Dig Lord [heavy]">
    <option value="Dominator [heavy]">
    <option value="Doom Courser [heavy]">
    <option value="Dragon [heavy]">
    <option value="Dragon Fire [heavy]">
    <option value="Dragon II [heavy]">
    <option value="Dragoon [heavy]">
    <option value="Excalibur [heavy]">
    <option value="Exterminator [heavy]">
    <option value="Falconer [heavy]">
    <option value="Fire Scorpion [heavy]">
    <option value="Flamberge [heavy]">
    <option value="Flashman [heavy]">
    <option value="Galahad [heavy]">
    <option value="Gallant [heavy]">
    <option value="Gallowglas [heavy]">
    <option value="Grand Dragon [heavy]">
    <option value="Grasshopper [heavy]">
    <option value="Grigori [heavy]">
    <option value="Grizzly [heavy]">
    <option value="Grommet [heavy]">
    <option value="Ground Pounder [heavy]">
    <option value="Guillotine [heavy]">
    <option value="Guillotine IIC [heavy]">
    <option value="Götterdämmerung [heavy]">
    <option value="Ha Otoko [heavy]">
    <option value="Hachiwara [heavy]">
    <option value="Hammerhands [heavy]">
    <option value="Harpagos [heavy]">
    <option value="Heavy Forester [heavy]">
    <option value="Heavy Lifter [heavy]">
    <option value="Hector [heavy]">
    <option value="Helepolis [heavy]">
    <option value="Helios [heavy]">
    <option value="Hellfire [heavy]">
    <option value="Hercules [heavy]">
    <option value="Hound [heavy]">
    <option value="Hybrid Rifleman [heavy]">
    <option value="Inferno [heavy]">
    <option value="Jade Hawk [heavy]">
    <option value="JagerMech [heavy]">
    <option value="JagerMech III [heavy]">
    <option value="Jinggau [heavy]">
    <option value="Karhu [heavy]">
    <option value="Koschei [heavy]">
    <option value="Kuma [heavy]">
    <option value="Lament [heavy]">
    <option value="Lancelot [heavy]">
    <option value="Lao Hu [heavy]">
    <option value="Lightning [heavy]">
    <option value="Linebacker [heavy]">
    <option value="Loki [heavy]">
    <option value="Loki Mk II [heavy]">
    <option value="Loki Mk II (Hel) [heavy]">
    <option value="Lumberjack [heavy]">
    <option value="Lupus [heavy]">
    <option value="Mad Cat [heavy]">
    <option value="Mad Cat Mk IV [heavy]">
    <option value="Mad Cat Mk IV PR [heavy]">
    <option value="Maelstrom [heavy]">
    <option value="Mangonel [heavy]">
    <option value="Marauder [heavy]">
    <option value="Masauwu [heavy]">
    <option value="Matador [heavy]">
    <option value="Merlin [heavy]">
    <option value="Minsk [heavy]">
    <option value="Morpheus [heavy]">
    <option value="Mortis [heavy]">
    <option value="MuckRaker [heavy]">
    <option value="Night Gyr [heavy]">
    <option value="Ninja-To [heavy]">
    <option value="No-Dachi [heavy]">
    <option value="Notos [heavy]">
    <option value="Nova Cat [heavy]">
    <option value="OmniMarauder [heavy]">
    <option value="Onslaught [heavy]">
    <option value="Orion [heavy]">
    <option value="Orion IIC [heavy]">
    <option value="Ostroc [heavy]">
    <option value="Ostsol [heavy]">
    <option value="Ostwar [heavy]">
    <option value="Paladin [heavy]">
    <option value="Pandarus [heavy]">
    <option value="Patriot [heavy]">
    <option value="Penetrator [heavy]">
    <option value="Penthesilea [heavy]">
    <option value="Perseus [heavy]">
    <option value="Predator [heavy]">
    <option value="Prefect [heavy]">
    <option value="Prometheus [heavy]">
    <option value="Quickdraw [heavy]">
    <option value="Rakshasa [heavy]">
    <option value="Reconquista [heavy]">
    <option value="Redback [heavy]">
    <option value="Rifleman [heavy]">
    <option value="Rifleman IIC [heavy]">
    <option value="Roughneck [heavy]">
    <option value="Ryoken II [heavy]">
    <option value="Scourge [heavy]">
    <option value="Shadow Cat II [heavy]">
    <option value="Shen Yi [heavy]">
    <option value="Shiro [heavy]">
    <option value="Shootist [heavy]">
    <option value="Shugenja [heavy]">
    <option value="Sidewinder [heavy]">
    <option value="Sojourner [heavy]">
    <option value="Spatha [heavy]">
    <option value="Sphinx [heavy]">
    <option value="Spirit Walker [heavy]">
    <option value="Starhawk [heavy]">
    <option value="Sun Spider [heavy]">
    <option value="Super-Griffin [heavy]">
    <option value="Temax Cat Ninjabolt [heavy]">
    <option value="Tempest [heavy]">
    <option value="Thanatos [heavy]">
    <option value="Thor [heavy]">
    <option value="Thor II [heavy]">
    <option value="Thresher [heavy]">
    <option value="Thresher Mk II [heavy]">
    <option value="Thunder [heavy]">
    <option value="Thunderbolt [heavy]">
    <option value="Thunderbolt IIC [heavy]">
    <option value="Ti Ts&#x27;ang [heavy]">
    <option value="Tian-Zong [heavy]">
    <option value="Toyama [heavy]">
    <option value="Triskelion [heavy]">
    <option value="Tundra Wolf [heavy]">
    <option value="Uni [heavy]">
    <option value="Uraeus [heavy]">
    <option value="UrbanKnight [heavy]">
    <option value="Urbanmaster [heavy]">
    <option value="Ursa [heavy]">
    <option value="Vandal [heavy]">
    <option value="Verfolger [heavy]">
    <option value="Viper [heavy]">
    <option value="Vision Quest [heavy]">
    <option value="Von Rohrs (Hebi) [heavy]">
    <option value="Vulpes [heavy]">
    <option value="Vulture [heavy]">
    <option value="Vulture Mk III [heavy]">
    <option value="Vulture Mk IV [heavy]">
    <option value="War Crow [heavy]">
    <option value="War Dog [heavy]">
    <option value="Warhammer [heavy]">
    <option value="Warwolf [heavy]">
    <option value="White Flame [heavy]">
    <option value="White Raven [heavy]">
    <option value="Wildfire [heavy]">
    <option value="Woodsman [heavy]">
    <option value="Yeoman [heavy]">
    <option value="&#x27;Gestalt&#x27; [medium]">
    <option value="&#x27;Wing&#x27; Wraith [medium]">
    <option value="Agrotera [medium]">
    <option value="Alfar [medium]">
    <option value="Antlion [medium]">
    <option value="Apollo [medium]">
    <option value="Aquagladius [medium]">
    <option value="Araña [medium]">
    <option value="Arctic Wolf [medium]">
    <option value="Arctic Wolf II [medium]">
    <option value="Assassin [medium]">
    <option value="Avalanche [medium]">
    <option value="Bakeneko [medium]">
    <option value="Battle Cobra [medium]">
    <option value="Beowulf [medium]">
    <option value="Beowulf IIC [medium]">
    <option value="Bishamon [medium]">
    <option value="Black Hawk [medium]">
    <option value="Black Hawk (Standard) [medium]">
    <option value="Black Lanner [medium]">
    <option value="Blackjack [medium]">
    <option value="Blitzkrieg [medium]">
    <option value="Bloodhound [medium]">
    <option value="Blue Flame [medium]">
    <option value="Bombard [medium]">
    <option value="Buccaneer [medium]">
    <option value="Bushwacker [medium]">
    <option value="Buster [medium]">
    <option value="Calliope [medium]">
    <option value="Carrion Crow [medium]">
    <option value="Centurion [medium]">
    <option value="Chameleon [medium]">
    <option value="Chimera [medium]">
    <option value="Cicada [medium]">
    <option value="Clint [medium]">
    <option value="Clint IIC [medium]">
    <option value="Cobra [medium]">
    <option value="Corvis [medium]">
    <option value="Coyotl [medium]">
    <option value="Crab [medium]">
    <option value="Crimson Langur [medium]">
    <option value="Cronus [medium]">
    <option value="Cuirass [medium]">
    <option value="Cyllaros [medium]">
    <option value="Daedalus [medium]">
    <option value="Daimyo [medium]">
    <option value="Dark Crow [medium]">
    <option value="Dasher II [medium]">
    <option value="Dervish [medium]">
    <option value="Dragonfly [medium]">
    <option value="Eidolon [medium]">
    <option value="Eisenfaust [medium]">
    <option value="Enfield [medium]">
    <option value="Enforcer [medium]">
    <option value="Enforcer III [medium]">
    <option value="Eris [medium]">
    <option value="Exhumer [medium]">
    <option value="Eyleuka [medium]">
    <option value="Fennec [medium]">
    <option value="Fenris [medium]">
    <option value="Firestorm [medium]">
    <option value="Fox [medium]">
    <option value="Fujin [medium]">
    <option value="Gauntlet [medium]">
    <option value="Gauss-Buster [medium]">
    <option value="Ghost [medium]">
    <option value="Goshawk [medium]">
    <option value="Goshawk II [medium]">
    <option value="Gravedigger [medium]">
    <option value="Great Wyrm [medium]">
    <option value="Grendel [medium]">
    <option value="Griffin [medium]">
    <option value="Griffin IIC [medium]">
    <option value="Grim Reaper [medium]">
    <option value="Gyrfalcon [medium]">
    <option value="Hammerhead [medium]">
    <option value="Harvester [medium]">
    <option value="Hatchetman [medium]">
    <option value="Hel [medium]">
    <option value="Hellcat [medium]">
    <option value="Hellhound [medium]">
    <option value="Hellhound II-P [medium]">
    <option value="Hellspawn [medium]">
    <option value="Hermes II [medium]">
    <option value="Hermes III [medium]">
    <option value="Hierofalcon [medium]">
    <option value="Hitotsume Kozo [medium]">
    <option value="Hollander II [medium]">
    <option value="Hoplite [medium]">
    <option value="Hunchback [medium]">
    <option value="Hunchback IIC [medium]">
    <option value="Huron Warrior [medium]">
    <option value="Hyena [medium]">
    <option value="Icarus [medium]">
    <option value="Icarus II [medium]">
    <option value="Initiate [medium]">
    <option value="Inquisitor II [medium]">
    <option value="Jabberwocky [medium]">
    <option value="Kheper [medium]">
    <option value="Kintaro [medium]">
    <option value="Komodo [medium]">
    <option value="Kontio [medium]">
    <option value="Kyudo [medium]">
    <option value="Legionnaire [medium]">
    <option value="Liberator [medium]">
    <option value="Lightray [medium]">
    <option value="Lineholder [medium]">
    <option value="Lobo [medium]">
    <option value="Lynx [medium]">
    <option value="Mad Cat III [medium]">
    <option value="Marshal [medium]">
    <option value="Men Shen [medium]">
    <option value="Mercury II [medium]">
    <option value="Mongoose II [medium]">
    <option value="Mongrel [medium]">
    <option value="Naja [medium]">
    <option value="Night Chanter [medium]">
    <option value="Night Stalker [medium]">
    <option value="Nightsky [medium]">
    <option value="Nobori-nin [medium]">
    <option value="Omni-Corvis [medium]">
    <option value="Osprey [medium]">
    <option value="Pariah (Septicemia) [medium]">
    <option value="Phantom [medium]">
    <option value="Phoenix [medium]">
    <option value="Phoenix Hawk [medium]">
    <option value="Phoenix Hawk &#x27;Hammer Hawk&#x27; [medium]">
    <option value="Phoenix Hawk LAM [medium]">
    <option value="Phoenix Hawk LAM Mk I [medium]">
    <option value="Pinion [medium]">
    <option value="Pouncer [medium]">
    <option value="Preta [medium]">
    <option value="Prowler [medium]">
    <option value="Quasimodo [medium]">
    <option value="Quasit [medium]">
    <option value="Rabid Coyote [medium]">
    <option value="Raider [medium]">
    <option value="Raider Mk II [medium]">
    <option value="Raijin [medium]">
    <option value="Raijin II [medium]">
    <option value="Raptor II [medium]">
    <option value="Rattlesnake II [medium]">
    <option value="Raven II [medium]">
    <option value="Rawhide [medium]">
    <option value="Rhino [medium]">
    <option value="Rime Otter [medium]">
    <option value="Rock Hound [medium]">
    <option value="Ronin [medium]">
    <option value="Rook [medium]">
    <option value="Ryoken [medium]">
    <option value="Ryoken III [medium]">
    <option value="Ryoken III-XP [medium]">
    <option value="Sarath [medium]">
    <option value="Sarissa [medium]">
    <option value="Scarecrow [medium]">
    <option value="Scorpion [medium]">
    <option value="Screamer LAM [medium]">
    <option value="Sentinel [medium]">
    <option value="Sentry [medium]">
    <option value="Sha Yu [medium]">
    <option value="Shadow Cat [medium]">
    <option value="Shadow Cat III [medium]">
    <option value="Shadow Hawk [medium]">
    <option value="Shadow Hawk IIC [medium]">
    <option value="Shadow Hawk LAM [medium]">
    <option value="Shockwave [medium]">
    <option value="Slagmaiden [medium]">
    <option value="Snake [medium]">
    <option value="Space Hound [medium]">
    <option value="Stag [medium]">
    <option value="Stag II [medium]">
    <option value="Stalking Spider [medium]">
    <option value="Stalking Spider II [medium]">
    <option value="Starslayer [medium]">
    <option value="Stealth [medium]">
    <option value="Stooping Hawk [medium]">
    <option value="Stormwolf [medium]">
    <option value="Strider [medium]">
    <option value="Sun Bear [medium]">
    <option value="Sun Cobra [medium]">
    <option value="Surtur [medium]">
    <option value="Swordsman [medium]">
    <option value="Talos [medium]">
    <option value="Targe [medium]">
    <option value="Tessen [medium]">
    <option value="Thunder Fox [medium]">
    <option value="Tolva [medium]">
    <option value="Trebuchet [medium]">
    <option value="Tsunami [medium]">
    <option value="Ursus [medium]">
    <option value="Ursus II [medium]">
    <option value="Uziel [medium]">
    <option value="Vindicator [medium]">
    <option value="Violator [medium]">
    <option value="Volkh [medium]">
    <option value="Vulcan [medium]">
    <option value="Waneta [medium]">
    <option value="Watchman [medium]">
    <option value="Wendigo [medium]">
    <option value="Wendigo-VP [medium]">
    <option value="Wendigo-XR [medium]">
    <option value="Werewolf [medium]">
    <option value="Whitworth [medium]">
    <option value="Wolf Trap (Tora) [medium]">
    <option value="Wolverine [medium]">
    <option value="Wolverine II [medium]">
    <option value="Wraith [medium]">
    <option value="Wyvern [medium]">
    <option value="Wyvern IIC [medium]">
    <option value="Yao Lien [medium]">
    <option value="Yurei [medium]">
    <option value="&#x27;Scout Tripod&#x27; [light]">
    <option value="Anubis [light]">
    <option value="Arbalest [light]">
    <option value="Arbiter [light]">
    <option value="Arctic Fox [light]">
    <option value="Arion [light]">
    <option value="Baboon [light]">
    <option value="Battle Hawk [light]">
    <option value="Bear Cub [light]">
    <option value="Blade [light]">
    <option value="Brigand [light]">
    <option value="Butcherbird [light]">
    <option value="Cadaver [light]">
    <option value="Carbine [light]">
    <option value="CattleMaster [light]">
    <option value="Cazador [light]">
    <option value="Cephalus [light]">
    <option value="Commando [light]">
    <option value="Commando IIC [light]">
    <option value="Copper [light]">
    <option value="Copperhead [light]">
    <option value="Cossack [light]">
    <option value="Cougar [light]">
    <option value="Cricket [light]">
    <option value="Crimson Hawk [light]">
    <option value="Crosscut [light]">
    <option value="Dart [light]">
    <option value="Dasher [light]">
    <option value="Demeter [light]">
    <option value="DemolitionMech [light]">
    <option value="Devil [light]">
    <option value="Dig King [light]">
    <option value="Dola [light]">
    <option value="Drift Shag [light]">
    <option value="Duan Gung [light]">
    <option value="Eagle [light]">
    <option value="Ebony [light]">
    <option value="Eyrie [light]">
    <option value="Falcon [light]">
    <option value="Falcon Hawk [light]">
    <option value="Fire Falcon [light]">
    <option value="Fireball [light]">
    <option value="Firebee [light]">
    <option value="Firefly [light]">
    <option value="Firestarter [light]">
    <option value="Flashfire [light]">
    <option value="Flea [light]">
    <option value="ForestryMech [light]">
    <option value="Fwltur [light]">
    <option value="Gambit [light]">
    <option value="Garm [light]">
    <option value="Gulon [light]">
    <option value="Gunsmith [light]">
    <option value="Gurkha [light]">
    <option value="Gùn [light]">
    <option value="Hammer [light]">
    <option value="Hankyu [light]">
    <option value="Harvester Ant [light]">
    <option value="Havoc [light]">
    <option value="Hellion [light]">
    <option value="Hermes [light]">
    <option value="Hermit Crab [light]">
    <option value="Hitman [light]">
    <option value="Hollander [light]">
    <option value="Hollander III [light]">
    <option value="Hornet [light]">
    <option value="Hussar [light]">
    <option value="Icestorm [light]">
    <option value="Incubus II [light]">
    <option value="Inquisitor [light]">
    <option value="Jackal [light]">
    <option value="Jackalope [light]">
    <option value="Jackrabbit [light]">
    <option value="Jaguar [light]">
    <option value="Javelin [light]">
    <option value="Jenner [light]">
    <option value="Jenner IIC [light]">
    <option value="Kabuto [light]">
    <option value="Koshi [light]">
    <option value="Koshi (Standard) [light]">
    <option value="Koto [light]">
    <option value="Locust [light]">
    <option value="Locust IIC [light]">
    <option value="Longshot [light]">
    <option value="Malak [light]">
    <option value="Mandrill [light]">
    <option value="Mantis [light]">
    <option value="Marco [light]">
    <option value="Mercury [light]">
    <option value="Mjolnir [light]">
    <option value="Mongoose [light]">
    <option value="Morrigan [light]">
    <option value="Nexus [light]">
    <option value="Nexus II [light]">
    <option value="Night Hawk [light]">
    <option value="Nyx [light]">
    <option value="Ocelot [light]">
    <option value="Opossum [light]">
    <option value="Osiris [light]">
    <option value="Ostscout [light]">
    <option value="Ostscout IIC [light]">
    <option value="Owens [light]">
    <option value="Pacifier [light]">
    <option value="Pack Hunter [light]">
    <option value="Pack Hunter II [light]">
    <option value="Panther [light]">
    <option value="Parash [light]">
    <option value="Pathfinder [light]">
    <option value="Peacemaker [light]">
    <option value="Peregrine [light]">
    <option value="Phoenix Hawk L &#x27;Fenikkusu Taka&#x27; [light]">
    <option value="Piranha [light]">
    <option value="Porcupine [light]">
    <option value="Powerman [light]">
    <option value="Puma [light]">
    <option value="Pwwka [light]">
    <option value="Raptor [light]">
    <option value="Rattlesnake [light]">
    <option value="Raven [light]">
    <option value="Raven X [light]">
    <option value="Razorback [light]">
    <option value="Red Shift [light]">
    <option value="Reptar [light]">
    <option value="Revenant [light]">
    <option value="Rokurokubi [light]">
    <option value="Scarabus [light]">
    <option value="Shugosha [light]">
    <option value="Silver Fox [light]">
    <option value="Sling [light]">
    <option value="Snow Fox [light]">
    <option value="Snow Fox (Omni) [light]">
    <option value="Sokuryou [light]">
    <option value="Solitaire [light]">
    <option value="Spector [light]">
    <option value="Spider [light]">
    <option value="Spindrift Aquatic SecurityMech [light]">
    <option value="Spirit [light]">
    <option value="Star Python [light]">
    <option value="Stiletto [light]">
    <option value="Stinger [light]">
    <option value="Stinger IIC [light]">
    <option value="Stinger LAM [light]">
    <option value="Stinger LAM Mk I [light]">
    <option value="Storm Raider [light]">
    <option value="StrongArm [light]">
    <option value="SuburbanMech [light]">
    <option value="Super-Wasp [light]">
    <option value="Talon [light]">
    <option value="Tarantula [light]">
    <option value="Thorn [light]">
    <option value="Tiburon [light]">
    <option value="Toro [light]">
    <option value="Trooper [light]">
    <option value="Uller [light]">
    <option value="UrbanMech [light]">
    <option value="UrbanMech IIC [light]">
    <option value="UrbanMech LAM [light]">
    <option value="Valiant [light]">
    <option value="Valkyrie [light]">
    <option value="Venom [light]">
    <option value="Vixen [light]">
    <option value="Wasp [light]">
    <option value="Wasp LAM [light]">
    <option value="Wasp LAM Mk I [light]">
    <option value="Wight [light]">
    <option value="Wolfhound [light]">
    <option value="Wolfhound IIC [light]">
    <option value="Wulfen [light]">
    <option value="Yinghuochong [light]">
    <option value="Basilisk [ultralight]">
    <option value="Basilisk ProtoMech (Quad) [ultralight]">
    <option value="Boggart Ultraheavy ProtoMech [ultralight]">
    <option value="Cecerops [ultralight]">
    <option value="Celerity [ultralight]">
    <option value="Centaur [ultralight]">
    <option value="Chaffee [ultralight]">
    <option value="Chrysaor [ultralight]">
    <option value="Delphyne [ultralight]">
    <option value="Erinyes [ultralight]">
    <option value="Exo [ultralight]">
    <option value="Gorgon [ultralight]">
    <option value="Guard [ultralight]">
    <option value="Harpy [ultralight]">
    <option value="Hippogriff [ultralight]">
    <option value="Hobgoblin Ultraheavy ProtoMech [ultralight]">
    <option value="Hydra [ultralight]">
    <option value="Minotaur [ultralight]">
    <option value="Orc [ultralight]">
    <option value="Patron [ultralight]">
    <option value="Pompier [ultralight]">
    <option value="Prey Seeker [ultralight]">
    <option value="Procyon [ultralight]">
    <option value="Roadrunner [ultralight]">
    <option value="Roc [ultralight]">
    <option value="Satyr [ultralight]">
    <option value="Siren [ultralight]">
    <option value="Sprite Ultraheavy ProtoMech [ultralight]">
    <option value="Svartalfa [ultralight]">
    <option value="Triton [ultralight]">
    <option value="AC/2 Carrier [vehicle]">
    <option value="Adelante Passenger/Cargo Train [vehicle]">
    <option value="Aeron Strike VTOL [vehicle]">
    <option value="Aesir Medium AA Vehicle [vehicle]">
    <option value="Air Car [vehicle]">
    <option value="Aithon Assault Transport [vehicle]">
    <option value="Ajax Assault Tank [vehicle]">
    <option value="Alacorn Heavy Tank [vehicle]">
    <option value="Anat APC [vehicle]">
    <option value="Anhur [vehicle]">
    <option value="Anhur Transport [vehicle]">
    <option value="Apocalypse World Rover [vehicle]">
    <option value="Apostle Self-Propelled Artillery [vehicle]">
    <option value="Apple-Churchill Surveillance VTOL [vehicle]">
    <option value="Ares Medium Tank [vehicle]">
    <option value="Armored Personnel Carrier [vehicle]">
    <option value="Arrow IV Assault Vehicle [vehicle]">
    <option value="Arrow IV Carrier [vehicle]">
    <option value="Asher Hover Scout [vehicle]">
    <option value="Asshur Artillery Spotter [vehicle]">
    <option value="Asshur Fast Reconnaissance Vehicle [vehicle]">
    <option value="Assuan Armored Bike [vehicle]">
    <option value="Aston-Martin Fiver Roadster [vehicle]">
    <option value="Aston-Martin Fiver Traveler [vehicle]">
    <option value="Athena Combat Vehicle [vehicle]">
    <option value="Atlantia Luxury Yacht [vehicle]">
    <option value="Augustus MBT [vehicle]">
    <option value="Auxiliary Defense Tank [vehicle]">
    <option value="Axel Heavy Tank [vehicle]">
    <option value="Axel Heavy Tank IIC [vehicle]">
    <option value="Badger (C) Tracked Transport [vehicle]">
    <option value="Badger Tracked Transport [vehicle]">
    <option value="Bailey Armored Car [vehicle]">
    <option value="Balac Strike VTOL [vehicle]">
    <option value="Baleena Passenger Submarine [vehicle]">
    <option value="Ballista Artillery Trailer [vehicle]">
    <option value="Ballista Self-Propelled Artillery Tank [vehicle]">
    <option value="Bandit (C) Hovercraft [vehicle]">
    <option value="Bandit Hovercraft [vehicle]">
    <option value="Bardiche Heavy Strike Tank [vehicle]">
    <option value="Barouche Military Transport [vehicle]">
    <option value="BattleMech Recovery Vehicle [vehicle]">
    <option value="Bayamo Hoverbike [vehicle]">
    <option value="Beagle Hover Scout [vehicle]">
    <option value="Beast Riot Car [vehicle]">
    <option value="Behemoth Heavy Tank [vehicle]">
    <option value="Behemoth II Heavy Tank [vehicle]">
    <option value="Bellona Hover Tank [vehicle]">
    <option value="Bishop Transport VTOL [vehicle]">
    <option value="Blackstone Baronet Passenger VTOL [vehicle]">
    <option value="Blackstone Pegasus Passenger VTOL [vehicle]">
    <option value="Blizzard Hover Transport [vehicle]">
    <option value="Bokkusu Support Trailer [vehicle]">
    <option value="Bolla Stealth Tank [vehicle]">
    <option value="Bolla Stealth Tank (RotS) [vehicle]">
    <option value="Bombduster [vehicle]">
    <option value="Boreas Cavalry Hovercraft [vehicle]">
    <option value="Browning Mobile HQ [vehicle]">
    <option value="Brunel Dump Truck [vehicle]">
    <option value="Brutus Assault Tank [vehicle]">
    <option value="Buffalo [vehicle]">
    <option value="Bulldog Medium Tank [vehicle]">
    <option value="Bulldog Medium Truck [vehicle]">
    <option value="Bulwark Assault Vehicle [vehicle]">
    <option value="Burke Defense Tank [vehicle]">
    <option value="Burke II Superheavy Tank [vehicle]">
    <option value="Burro Heavy Support Truck [vehicle]">
    <option value="Burro II Super Heavy Cargo Truck [vehicle]">
    <option value="Buzzard Hover Tank [vehicle]">
    <option value="Büffel Engineering Support Vehicle VII [vehicle]">
    <option value="Büffel Engineering Support Vehicle VIII [vehicle]">
    <option value="C-85 Sprinter Delivery Vehicle [vehicle]">
    <option value="Caravan Heavy Transport [vehicle]">
    <option value="Cardinal Transport [vehicle]">
    <option value="Carnivore Assault Tank [vehicle]">
    <option value="Carter Medical Emergency Response Vehicle [vehicle]">
    <option value="Cascatelle Firefighting VTOL [vehicle]">
    <option value="Cavalry Attack Helicopter [vehicle]">
    <option value="Cellco Ranger [vehicle]">
    <option value="Centipede Scout Car [vehicle]">
    <option value="Ceres-Bikes Flashbang [vehicle]">
    <option value="Chalchiuhtotolin Support Tank [vehicle]">
    <option value="Challenger MBT [vehicle]">
    <option value="Champion Hoversports Crimson Streak Hover Racer [vehicle]">
    <option value="Champion Hoversports CS535 Hover Car [vehicle]">
    <option value="Chaparral Missile Artillery Tank [vehicle]">
    <option value="Chevalier Light Tank [vehicle]">
    <option value="Chi-Ha Infantry Combat Vehicle [vehicle]">
    <option value="Cizin Hover Tank [vehicle]">
    <option value="Coanda Personal Sports Craft [vehicle]">
    <option value="Cobra Transport VTOL [vehicle]">
    <option value="Cobra VTOL Transport [vehicle]">
    <option value="Command Van [vehicle]">
    <option value="Condor Heavy Hover Tank [vehicle]">
    <option value="Condor HoverBall [vehicle]">
    <option value="Condor Multi-Purpose Tank [vehicle]">
    <option value="Coolant Truck [vehicle]">
    <option value="Cormorant Medium Cargo Craft [vehicle]">
    <option value="Corx Mobile Tunnel Miner [vehicle]">
    <option value="Crane Heavy Transport [vehicle]">
    <option value="Crow Scout Helicopter [vehicle]">
    <option value="Croyle Systems Cortez Series N [vehicle]">
    <option value="Cyrano Gunship [vehicle]">
    <option value="Daimyo HQ [vehicle]">
    <option value="Danai Support Vehicle [vehicle]">
    <option value="Darter Scout Car [vehicle]">
    <option value="Death Trike [vehicle]">
    <option value="Debbie [vehicle]">
    <option value="Demolisher Heavy Tank [vehicle]">
    <option value="Demolisher II Heavy Tank [vehicle]">
    <option value="Demon Medium Tank [vehicle]">
    <option value="Demon Tank [vehicle]">
    <option value="Desert Scorpion Light Tank [vehicle]">
    <option value="Destrier Siege Vehicle [vehicle]">
    <option value="Deusenberg VIP Luxury Hovercar [vehicle]">
    <option value="Devastator Heavy Tank [vehicle]">
    <option value="Devastator II Superheavy Tank [vehicle]">
    <option value="DI Morgan Assault Tank [vehicle]">
    <option value="DI Multipurpose VTOL [vehicle]">
    <option value="DI Schmitt Tank [vehicle]">
    <option value="Diggs Drone Control Tank [vehicle]">
    <option value="Dillinger Police Vehicle [vehicle]">
    <option value="Donar Assault Helicopter [vehicle]">
    <option value="Dreadnought Mk II Land Trailer [vehicle]">
    <option value="Dreadnought Mk II Land Train [vehicle]">
    <option value="Drillson Heavy Hover Tank [vehicle]">
    <option value="Dromedary Water Transport [vehicle]">
    <option value="Dune Buggy [vehicle]">
    <option value="Dunning Mobile Tactical Command Post [vehicle]">
    <option value="Durandel-British &#x27;Blue Nova&#x27; Convertible [vehicle]">
    <option value="Eagre Firefighting ATV [vehicle]">
    <option value="Eldingar Hover Sled [vehicle]">
    <option value="Engineering Vehicle [vehicle]">
    <option value="Enyo Strike Tank [vehicle]">
    <option value="Epona Pursuit Tank [vehicle]">
    <option value="Estevez MBT [vehicle]">
    <option value="Eurus MBT [vehicle]">
    <option value="Falcon Hover Tank [vehicle]">
    <option value="Fensalir Combat WiGE [vehicle]">
    <option value="Ferret Light Scout VTOL [vehicle]">
    <option value="Flatbed Railcar [vehicle]">
    <option value="Flatbed Truck [vehicle]">
    <option value="Fortune Wheeled Assault Vehicle [vehicle]">
    <option value="Fox Armored Car [vehicle]">
    <option value="Freedom 900 Hover Jeep [vehicle]">
    <option value="Fulcrum Heavy Hovertank [vehicle]">
    <option value="Fulcrum Heavy HoverTank [vehicle]">
    <option value="Fulmar Patrol Craft [vehicle]">
    <option value="Fury Command Tank [vehicle]">
    <option value="Gabriel Reconnaissance Hovercraft [vehicle]">
    <option value="Galaport Ground Trailer [vehicle]">
    <option value="Galaport Ground Tug [vehicle]">
    <option value="Gallant Urban Assault Tank [vehicle]">
    <option value="Galleon Light Tank [vehicle]">
    <option value="Garrot Superheavy Transport [vehicle]">
    <option value="Garuda Heavy VTOL [vehicle]">
    <option value="Generic Expandable Services Vehicle [vehicle]">
    <option value="Generic Expandable Services Vehicle Trailer [vehicle]">
    <option value="Giant Guardian [vehicle]">
    <option value="Gienah-Durapaq Elite Land Train [vehicle]">
    <option value="Giggins APC [vehicle]">
    <option value="Gladius Medium Hover Tank [vehicle]">
    <option value="Glaive Medium Tank [vehicle]">
    <option value="Glory Heavy Fire Support Vehicle [vehicle]">
    <option value="Goblin II Infantry Support Vehicle [vehicle]">
    <option value="Goblin III Infantry Support Vehicle [vehicle]">
    <option value="Goblin Infantry Support Vehicle [vehicle]">
    <option value="Goblin Medium Tank [vehicle]">
    <option value="Gossamer VTOL [vehicle]">
    <option value="Ground Car [vehicle]">
    <option value="Gulltoppr OmniMonitor [vehicle]">
    <option value="Gun Trailer [vehicle]">
    <option value="Gurzil Support Tank [vehicle]">
    <option value="Gürteltier MBT [vehicle]">
    <option value="Hachiman Fire Support Tank [vehicle]">
    <option value="Hadur Fast Support Vehicle [vehicle]">
    <option value="Hanse MBT [vehicle]">
    <option value="Harasser Laser Platform [vehicle]">
    <option value="Harasser Missile Platform [vehicle]">
    <option value="Harpoon ParaSub [vehicle]">
    <option value="Harrier Heavy Hover Tank [vehicle]">
    <option value="Hasek Mechanized Combat Vehicle [vehicle]">
    <option value="Hawk Hover Tank [vehicle]">
    <option value="Hawk Moth Gunship [vehicle]">
    <option value="Hawk Moth II Gunship [vehicle]">
    <option value="Heavy BattleMech Recovery Vehicle [vehicle]">
    <option value="Heavy Combat ATV [vehicle]">
    <option value="Heavy Hover APC [vehicle]">
    <option value="Heavy LRM Carrier [vehicle]">
    <option value="Heavy MML Carrier [vehicle]">
    <option value="Heavy NLRM Carrier [vehicle]">
    <option value="Heavy Tracked APC [vehicle]">
    <option value="Heavy Transport B1 [vehicle]">
    <option value="Heavy Weapons Carrier [vehicle]">
    <option value="Heavy Wheeled APC [vehicle]">
    <option value="Hector Road Train Tractor [vehicle]">
    <option value="Hector Road Train Trailer Module [vehicle]">
    <option value="Heimdall Ground Monitor Tank [vehicle]">
    <option value="Hephaestus Jump Tank [vehicle]">
    <option value="Hephaestus Scout Tank [vehicle]">
    <option value="Hesiod Utility Vehicle [vehicle]">
    <option value="Hetzer Wheeled Assault Gun [vehicle]">
    <option value="Hexareme HQ Hovercraft [vehicle]">
    <option value="Hi-Scout Drone [vehicle]">
    <option value="Hi-Scout Drone Carrier [vehicle]">
    <option value="Hipparch Cavalry Tank [vehicle]">
    <option value="Hiryo Armored Infantry Transport [vehicle]">
    <option value="HMRV (Hazardous Materials Recovery Vehicle) [vehicle]">
    <option value="Hoodling Sensor HoverJeep [vehicle]">
    <option value="Hover Scout [vehicle]">
    <option value="Hover Tank [vehicle]">
    <option value="Hoverbike [vehicle]">
    <option value="HoverPod [vehicle]">
    <option value="HTE Micro-Copter [vehicle]">
    <option value="Huitzilopochtli Assault Tank &#x27;Huey&#x27; [vehicle]">
    <option value="Humming Bird VTOL [vehicle]">
    <option value="Hunter Light Support Tank [vehicle]">
    <option value="Hwacha Urban Combat Vehicle [vehicle]">
    <option value="Ibex RV [vehicle]">
    <option value="Ignis Infantry Support Tank [vehicle]">
    <option value="Ina-du Swamp Skimmer [vehicle]">
    <option value="Indra Infantry Transport [vehicle]">
    <option value="Ishtar Heavy Fire Support Tank [vehicle]">
    <option value="J-27 Ordnance Transport [vehicle]">
    <option value="J-27 Trailer [vehicle]">
    <option value="J-37 Ordnance Transport [vehicle]">
    <option value="J. Edgar Light Hover Tank [vehicle]">
    <option value="Jagdpanzer II [vehicle]">
    <option value="Jeep [vehicle]">
    <option value="JES I Tactical Missile Carrier [vehicle]">
    <option value="JES II Strategic Missile Carrier [vehicle]">
    <option value="JES III Missile Carrier [vehicle]">
    <option value="Jet Sled [vehicle]">
    <option value="JI-002 HoverBike [vehicle]">
    <option value="JI2A1 Attack APC [vehicle]">
    <option value="Jifty Transportable Field Repair Unit [vehicle]">
    <option value="Jonah Submarine [vehicle]">
    <option value="Joust Medium Tank [vehicle]">
    <option value="Kalki Cruise Missile Launcher [vehicle]">
    <option value="Kallon UL-series Construction Vehicle [vehicle]">
    <option value="Kamakiri Attack VTOL [vehicle]">
    <option value="Kamisori Light Tank [vehicle]">
    <option value="Kanga Medium Hovertank [vehicle]">
    <option value="Kanga-X Jump Tank [vehicle]">
    <option value="Karnov UR Gunship [vehicle]">
    <option value="Karnov UR Transport [vehicle]">
    <option value="Kelswa Assault Tank [vehicle]">
    <option value="Kestrel VTOL [vehicle]">
    <option value="Kinnol MBT [vehicle]">
    <option value="Kite Reconnaissance Vehicle [vehicle]">
    <option value="Knox Armored Car [vehicle]">
    <option value="Koi Heavy Transport [vehicle]">
    <option value="Kokou Defense Tank [vehicle]">
    <option value="Korvin Tank [vehicle]">
    <option value="Koryu Submarine [vehicle]">
    <option value="Kruger Combat Car [vehicle]">
    <option value="Ku Wheeled Assault Tank [vehicle]">
    <option value="Lama-Deux Firefighting VTOL [vehicle]">
    <option value="Lamprey Transport Helicopter [vehicle]">
    <option value="Laser Carrier [vehicle]">
    <option value="Lesseps Dump Truck [vehicle]">
    <option value="Lewis Skimmer Bus [vehicle]">
    <option value="Lexan Oceanic Luxury VTOL [vehicle]">
    <option value="Lexan Oceanic Personal VTOL [vehicle]">
    <option value="Lexan Surveillance Helo [vehicle]">
    <option value="Lexan Surveillance VTOL [vehicle]">
    <option value="Light SRM Carrier [vehicle]">
    <option value="Light Thunderbolt Carrier [vehicle]">
    <option value="Lightning Attack Hovercraft [vehicle]">
    <option value="Limpet Half-Track [vehicle]">
    <option value="LRM Carrier [vehicle]">
    <option value="Luciano White Wolverine [vehicle]">
    <option value="Luduan Scout Vehicle [vehicle]">
    <option value="Magellan Series Four [vehicle]">
    <option value="Magi Infantry Support Vehicle [vehicle]">
    <option value="Main Gauche Light Support Tank [vehicle]">
    <option value="Main Guardian [vehicle]">
    <option value="Mamono IFV [vehicle]">
    <option value="Manta Fast Attack Submarine [vehicle]">
    <option value="Manteuffel Attack Tank [vehicle]">
    <option value="Manticore Heavy Tank [vehicle]">
    <option value="Manticore II Heavy Tank [vehicle]">
    <option value="Mantis Light Attack VTOL [vehicle]">
    <option value="Mao-Heng Charioteer [vehicle]">
    <option value="Marksman Artillery Vehicle [vehicle]">
    <option value="Marksman MBT [vehicle]">
    <option value="Mars Assault Vehicle [vehicle]">
    <option value="Marsden MBT [vehicle]">
    <option value="Marten Scout VTOL [vehicle]">
    <option value="MASH Truck [vehicle]">
    <option value="Maultier Hover APC [vehicle]">
    <option value="Mauna Kea Command Vessel [vehicle]">
    <option value="Maxim (I) Heavy Hover Transport [vehicle]">
    <option value="Maxim Flanker [vehicle]">
    <option value="Maxim Heavy Hover Transport [vehicle]">
    <option value="Maxim Mk II Transport [vehicle]">
    <option value="Merkava Heavy Tank [vehicle]">
    <option value="MHI Amphibious APC [vehicle]">
    <option value="MHI Defense AA Tank [vehicle]">
    <option value="Minigun Cycle [vehicle]">
    <option value="Minion Advanced Tactical Vehicle [vehicle]">
    <option value="MIT23 MASH Vehicle [vehicle]">
    <option value="MIT24 MASH Vehicle [vehicle]">
    <option value="Mithras Light Tank [vehicle]">
    <option value="Mobile Headquarters [vehicle]">
    <option value="Mobile Long Tom Artillery [vehicle]">
    <option value="Moltke MBT [vehicle]">
    <option value="Monitor Naval Vessel [vehicle]">
    <option value="Monocycle [vehicle]">
    <option value="Moray Heavy Attack Submarine [vehicle]">
    <option value="Morningstar City Command Vehicle [vehicle]">
    <option value="Morrigu Fire Support Vehicle [vehicle]">
    <option value="Mortar Carrier [vehicle]">
    <option value="Mosquito Light Fighter [vehicle]">
    <option value="MRM Carrier [vehicle]">
    <option value="Musketeer Hover Tank [vehicle]">
    <option value="Myrmidon Medium Tank [vehicle]">
    <option value="Mótuö Chë Shang No.2 [vehicle]">
    <option value="Nacon Armored Scout [vehicle]">
    <option value="Narukami Heavy Tank [vehicle]">
    <option value="Neptune Submarine [vehicle]">
    <option value="Nifty Transportable Field Repair Unit [vehicle]">
    <option value="Nightshade ECM VTOL [vehicle]">
    <option value="Nike Air Defense Platform [vehicle]">
    <option value="Nishikigoi Support Aircraft [vehicle]">
    <option value="Nisos Attack WIGE [vehicle]">
    <option value="Nuberu Anti Aircraft Tank [vehicle]">
    <option value="Obuzaabaa Tactical Vehicle [vehicle]">
    <option value="Odin Scout Tank [vehicle]">
    <option value="Ontos Heavy Tank [vehicle]">
    <option value="Onuris Attack VTOL [vehicle]">
    <option value="Oro Heavy Tank [vehicle]">
    <option value="Outrider [vehicle]">
    <option value="Packrat LRPV [vehicle]">
    <option value="Padilla Anti-Missile Tank [vehicle]">
    <option value="Padilla Heavy Artillery Tank [vehicle]">
    <option value="Padilla Tube Artillery Tank [vehicle]">
    <option value="Paladin Defense System [vehicle]">
    <option value="Palmoni Assault Infantry Fighting Vehicle [vehicle]">
    <option value="Pandion Combat WiGE [vehicle]">
    <option value="Paramour Mobile Repair Vehicle [vehicle]">
    <option value="Partisan AA Vehicle [vehicle]">
    <option value="Partisan Air Defense Tank [vehicle]">
    <option value="Partisan Heavy Tank [vehicle]">
    <option value="Partisan Hull Defense [vehicle]">
    <option value="Patton Tank [vehicle]">
    <option value="Peacekeeper SWAT Carrier [vehicle]">
    <option value="Pegasus Scout Hover Tank [vehicle]">
    <option value="Peregrine Attack VTOL [vehicle]">
    <option value="Phalanx Support Tank [vehicle]">
    <option value="Pike Support Vehicle [vehicle]">
    <option value="Pilum Heavy Tank [vehicle]">
    <option value="Pinto Attack VTOL [vehicle]">
    <option value="Pit Bull Medium Truck [vehicle]">
    <option value="Pixiu Heavy Tank [vehicle]">
    <option value="Plainsman Medium Hovertank [vehicle]">
    <option value="Po Heavy Tank [vehicle]">
    <option value="Po II Heavy Tank [vehicle]">
    <option value="Pollux ADA Heavy Tank [vehicle]">
    <option value="Pollux II ADA Heavy Tank [vehicle]">
    <option value="Praetorian Mobile Strategic Command HQ [vehicle]">
    <option value="Prairie Schooner Land Train [vehicle]">
    <option value="Prairie Schooner Module [vehicle]">
    <option value="Predator Tank Destroyer [vehicle]">
    <option value="Prime Mover [vehicle]">
    <option value="Prometheus Combat Support Bridgelayer [vehicle]">
    <option value="Prowler Multi-Terrain Vehicle [vehicle]">
    <option value="Puma Assault Tank [vehicle]">
    <option value="Quaestor Mobile Tactical Command HQ [vehicle]">
    <option value="Quicksilver Personal Sports Craft [vehicle]">
    <option value="R10 Mechanized ICV [vehicle]">
    <option value="Randolph Support Vehicle [vehicle]">
    <option value="Ranger Armored Fighting Vehicle [vehicle]">
    <option value="Raptor RRV [vehicle]">
    <option value="Reaper Self-Propelled Artillery [vehicle]">
    <option value="Red Kite Attack VTOL [vehicle]">
    <option value="Regulator Hovertank [vehicle]">
    <option value="Regulator II Hovertank [vehicle]">
    <option value="Rhino Fire Support Tank [vehicle]">
    <option value="Ripper Infantry Transport [vehicle]">
    <option value="Rock Rover Half-Track [vehicle]">
    <option value="Rommel Tank [vehicle]">
    <option value="Rotunda Scout Vehicle [vehicle]">
    <option value="Routemaster Hover Shuttle [vehicle]">
    <option value="RR-3 Recovery Vehicle [vehicle]">
    <option value="Rumbler-HT [vehicle]">
    <option value="Ryu Heavy Transport [vehicle]">
    <option value="Sabaku Kaze Heavy Scout Hover Tank [vehicle]">
    <option value="Saladin Assault Hover Tank [vehicle]">
    <option value="Saladin Mk II HCV [vehicle]">
    <option value="Sand Devil Hover Tank [vehicle]">
    <option value="Saracen Medium Hover Tank [vehicle]">
    <option value="Saracen Mk II HCV [vehicle]">
    <option value="Sasayaku Control Transport [vehicle]">
    <option value="Saturn Harvester [vehicle]">
    <option value="Saturnus V Grande Circuit Racer [vehicle]">
    <option value="Saurer-Bucher Fire Engine [vehicle]">
    <option value="Savannah Master Hovercraft [vehicle]">
    <option value="Savior Repair Vehicle [vehicle]">
    <option value="Saxon APC [vehicle]">
    <option value="Scapha Hovertank [vehicle]">
    <option value="Schildkröte Line Tank [vehicle]">
    <option value="Schiltron Mobile Fire-Support Platform [vehicle]">
    <option value="Schrek AC Carrier [vehicle]">
    <option value="Schrek Gauss Carrier [vehicle]">
    <option value="Schrek II-X PPC Carrier [vehicle]">
    <option value="Schrek PPC Carrier [vehicle]">
    <option value="Scimitar Medium Hover Tank [vehicle]">
    <option value="Scimitar Mk II HCV [vehicle]">
    <option value="Scorpion Light Tank [vehicle]">
    <option value="Scout ATV [vehicle]">
    <option value="Sea Hunter Maritime Tank [vehicle]">
    <option value="Sea Skimmer Hydrofoil [vehicle]">
    <option value="Seahorse Cargo Sub [vehicle]">
    <option value="Sekhmet Assault Vehicle [vehicle]">
    <option value="Shackleton AESV [vehicle]">
    <option value="Shamash Reconnaissance Vehicle [vehicle]">
    <option value="Shandra Advanced Scout Vehicle [vehicle]">
    <option value="Sheriff Infantry Support Tank [vehicle]">
    <option value="Sherpa Armored Truck [vehicle]">
    <option value="Shillelagh Missile Tank [vehicle]">
    <option value="Shoden Assault Vehicle [vehicle]">
    <option value="Shun Transport VTOL [vehicle]">
    <option value="Silverback Coastal Cutter [vehicle]">
    <option value="Silverfin Coastal Cutter [vehicle]">
    <option value="Simca Ambulance [vehicle]">
    <option value="Skadi Swift Attack VTOL [vehicle]">
    <option value="Skanda Light Tank [vehicle]">
    <option value="Skimmer [vehicle]">
    <option value="Skoda &#x27;Growler&#x27; Service Utility Truck [vehicle]">
    <option value="Skulker Wheeled Scout Tank [vehicle]">
    <option value="Skulker Wheeled Scout Tank Mk II [vehicle]">
    <option value="Sky Eye News Helicopter [vehicle]">
    <option value="Sleipnir APC Tank [vehicle]">
    <option value="Slipper Hovercar LX-Series [vehicle]">
    <option value="SM Tank Destroyer [vehicle]">
    <option value="SM2 Heavy Artillery Vehicle [vehicle]">
    <option value="SM5 Field Commander [vehicle]">
    <option value="Small Steamer [vehicle]">
    <option value="Sniper Artillery [vehicle]">
    <option value="SOAR VTOL [vehicle]">
    <option value="Soarece Superheavy MBT [vehicle]">
    <option value="Sokar Urban Combat Unit [vehicle]">
    <option value="Sortek Assault Craft [vehicle]">
    <option value="Speeder [vehicle]">
    <option value="Sprint Scout Helicopter [vehicle]">
    <option value="SRM Carrier [vehicle]">
    <option value="St. Christopher Cargo Transport [vehicle]">
    <option value="Stoat Scout Car [vehicle]">
    <option value="Strike Falcon Attack VTOL [vehicle]">
    <option value="Striker Light Tank [vehicle]">
    <option value="Strix Stealth VTOL [vehicle]">
    <option value="Sturmblitz Assault Gun [vehicle]">
    <option value="SturmFeur &#x27;Kalki&#x27; Cruise Missile Launcher [vehicle]">
    <option value="SturmFeur Heavy Tank [vehicle]">
    <option value="Sturmvogel Gunship [vehicle]">
    <option value="Sturmvogel Maritime Patrol WiGE [vehicle]">
    <option value="Stygian Strike Tank [vehicle]">
    <option value="Svantovit Infantry Fighting Vehicle [vehicle]">
    <option value="Swallow Attack WiGE [vehicle]">
    <option value="Swallow Attack WIGE [vehicle]">
    <option value="Swift Guardian [vehicle]">
    <option value="Swift Wind Scout Car [vehicle]">
    <option value="Swiftran RTC-215M [vehicle]">
    <option value="Tamerlane Strike Sled [vehicle]">
    <option value="Tenmaku Command Trailer [vehicle]">
    <option value="Teppō Artillery Vehicle [vehicle]">
    <option value="Testudo Siege Tank [vehicle]">
    <option value="TGV Omni Railcar [vehicle]">
    <option value="Thang-Ta APC [vehicle]">
    <option value="Thor Artillery Vehicle [vehicle]">
    <option value="Thumper Artillery Vehicle [vehicle]">
    <option value="Tiger Medium Tank [vehicle]">
    <option value="Tokugawa Heavy Tank [vehicle]">
    <option value="Tonbo Superheavy Transport [vehicle]">
    <option value="TrackBike [vehicle]">
    <option value="Tracked Quad [vehicle]">
    <option value="Trajan Assault Infantry Fighting Vehicle [vehicle]">
    <option value="Transportable Field Repair Unit [vehicle]">
    <option value="Tribune Mobile Tactical Command HQ [vehicle]">
    <option value="Trike [vehicle]">
    <option value="Trireme Infantry Transport [vehicle]">
    <option value="Tufana Hovercraft [vehicle]">
    <option value="Turhan Urban Combat Vehicle [vehicle]">
    <option value="Typhoon Urban Assault Vehicle [vehicle]">
    <option value="Tyr Infantry Support Tank [vehicle]">
    <option value="UTR-588 Recovery Vehicle [vehicle]">
    <option value="Vali Artillery Vehicle [vehicle]">
    <option value="Vargr APC Tank [vehicle]">
    <option value="Vector Combat Support VTOL [vehicle]">
    <option value="Vedette Medium Tank [vehicle]">
    <option value="Vidar Heavy Defense Tank [vehicle]">
    <option value="Von Luckner Heavy Tank [vehicle]">
    <option value="Warrior Attack Helicopter [vehicle]">
    <option value="Warrior Stealth Helicopter [vehicle]">
    <option value="Wayland Mobile Base [vehicle]">
    <option value="Weapon Carrier (Tracked) [vehicle]">
    <option value="Weapons Carrier A [vehicle]">
    <option value="Weapons-Troop Carrier [vehicle]">
    <option value="Wheeled Scout [vehicle]">
    <option value="Whirlwind Scout Hover Tank [vehicle]">
    <option value="White Tip Submarine [vehicle]">
    <option value="Whitestreak Jetski [vehicle]">
    <option value="Whitestreak Speedboat [vehicle]">
    <option value="Winston Combat Vehicle [vehicle]">
    <option value="Winterhawk APC [vehicle]">
    <option value="Yasha VTOL [vehicle]">
    <option value="Yellow Jacket Gunship [vehicle]">
    <option value="Zahn Heavy Transport [vehicle]">
    <option value="Zephyr Hovertank [vehicle]">
    <option value="Zephyros Infantry Support Vehicle [vehicle]">
    <option value="Zhukov Heavy Tank [vehicle]">
    <option value="Zibler Fast Strike Tank [vehicle]">
    <option value="Zorya Light Tank [vehicle]">
    <option value="Zugvogel Omni Support Aircraft [vehicle]">
    <option value="&#x27;Astrolux&#x27; Staryacht [aerospace]">
    <option value="Achilles [aerospace]">
    <option value="Aegis Heavy Cruiser [aerospace]">
    <option value="Aeshna Heavy Drone Fighter [aerospace]">
    <option value="Aesir Assault DropShip [aerospace]">
    <option value="Agamemnon Heavy Cruiser [aerospace]">
    <option value="Ahab [aerospace]">
    <option value="Alliance Space Station [aerospace]">
    <option value="Ammon [aerospace]">
    <option value="Aquarius Escort [aerospace]">
    <option value="Aqueduct Liquid Carrier [aerospace]">
    <option value="Aquila [aerospace]">
    <option value="Aquilla Transport JumpShip [aerospace]">
    <option value="Arcadia [aerospace]">
    <option value="Ares Assault Craft [aerospace]">
    <option value="Ares Attack Craft [aerospace]">
    <option value="Ares Close Assault Landing Craft [aerospace]">
    <option value="Ares Landing Craft [aerospace]">
    <option value="Arondight Pocket WarShip [aerospace]">
    <option value="Assault Triumph [aerospace]">
    <option value="Athena Cruiser [aerospace]">
    <option value="Atreus Battleship [aerospace]">
    <option value="Aurora [aerospace]">
    <option value="Avalon Cruiser [aerospace]">
    <option value="Avar [aerospace]">
    <option value="Avatar Heavy Cruiser [aerospace]">
    <option value="Avenger [aerospace]">
    <option value="Banshee [aerospace]">
    <option value="Baron Destroyer [aerospace]">
    <option value="Bashkir [aerospace]">
    <option value="Bastion System Defense Station [aerospace]">
    <option value="Battle Taxi [aerospace]">
    <option value="BattleSat System Defense Station [aerospace]">
    <option value="Batu [aerospace]">
    <option value="Behemoth [aerospace]">
    <option value="Behemoth C [aerospace]">
    <option value="Black Eagle [aerospace]">
    <option value="Black Lion I Battlecruiser [aerospace]">
    <option value="Black Lion II Battlecruiser [aerospace]">
    <option value="Blackwasp [aerospace]">
    <option value="Bluehawk Combat Support Fighter [aerospace]">
    <option value="Boeing Jump Bomber [aerospace]">
    <option value="Bonaventure Corvette [aerospace]">
    <option value="Boomerang Spotter Plane [aerospace]">
    <option value="Broadsword [aerospace]">
    <option value="Buccaneer [aerospace]">
    <option value="Bug-Eye Surveillance Ship [aerospace]">
    <option value="Bullet Suicide Drone [aerospace]">
    <option value="Bus [aerospace]">
    <option value="Caerleon [aerospace]">
    <option value="Cameron Battlecruiser [aerospace]">
    <option value="Capital Drone [aerospace]">
    <option value="Capitol System Defense Station [aerospace]">
    <option value="Cargo King [aerospace]">
    <option value="Cargomaster [aerospace]">
    <option value="Carrack Transport [aerospace]">
    <option value="Carrier [aerospace]">
    <option value="Carson Destroyer [aerospace]">
    <option value="Castrum Pocket WarShip [aerospace]">
    <option value="Centurion [aerospace]">
    <option value="Chaeronea [aerospace]">
    <option value="Chariot High-Speed Medevac [aerospace]">
    <option value="Cheetah [aerospace]">
    <option value="Cheetah II [aerospace]">
    <option value="Cheetah IIC [aerospace]">
    <option value="Chimeisho JumpShip [aerospace]">
    <option value="Chippewa [aerospace]">
    <option value="Chippewa IIC [aerospace]">
    <option value="Claymore [aerospace]">
    <option value="Cockatrice Monitor Platform [aerospace]">
    <option value="Colossus [aerospace]">
    <option value="Colt Medium Fighter [aerospace]">
    <option value="Comet Airliner [aerospace]">
    <option value="Comitatus JumpShip [aerospace]">
    <option value="Commonwealth Light Cruiser [aerospace]">
    <option value="Concordat Frigate [aerospace]">
    <option value="Condor [aerospace]">
    <option value="Condottiere Assault Craft [aerospace]">
    <option value="Conestoga Transport JumpShip [aerospace]">
    <option value="Confederate [aerospace]">
    <option value="Confederate C [aerospace]">
    <option value="Congress D Frigate [aerospace]">
    <option value="Congress Frigate [aerospace]">
    <option value="Conqueror Battlecruiser Carrier [aerospace]">
    <option value="Conquistador [aerospace]">
    <option value="Corax [aerospace]">
    <option value="Corone Destroyer [aerospace]">
    <option value="Corsair [aerospace]">
    <option value="Crucible Station [aerospace]">
    <option value="Cruiser [aerospace]">
    <option value="Cutlass [aerospace]">
    <option value="Czar DropShip [aerospace]">
    <option value="Dagger [aerospace]">
    <option value="Danais [aerospace]">
    <option value="Dante Frigate [aerospace]">
    <option value="Dart Light Cruiser [aerospace]">
    <option value="Davion Destroyer [aerospace]">
    <option value="Deathstalker [aerospace]">
    <option value="Defender Battlecruiser [aerospace]">
    <option value="Defiance [aerospace]">
    <option value="Delta Air Cruiser [aerospace]">
    <option value="Dictator [aerospace]">
    <option value="Dragau Assault Interceptor [aerospace]">
    <option value="Dragau II [aerospace]">
    <option value="Dragau II Assault Interceptor [aerospace]">
    <option value="DragonFly [aerospace]">
    <option value="Dragonstar Assault Transport [aerospace]">
    <option value="Dragonstar Passenger Transport [aerospace]">
    <option value="Drake Medium Stealth Fighter [aerospace]">
    <option value="Drake Medium Strike Fighter [aerospace]">
    <option value="Drake SDS Control Station [aerospace]">
    <option value="Dreadnought Battleship [aerospace]">
    <option value="Drone [aerospace]">
    <option value="DropShuttle [aerospace]">
    <option value="DroST IIa Transport [aerospace]">
    <option value="DroST IIb Transport [aerospace]">
    <option value="Du Shi Wang Battleship [aerospace]">
    <option value="Duat Military Transport [aerospace]">
    <option value="Eagle [aerospace]">
    <option value="Eagle Frigate [aerospace]">
    <option value="Eisensturm [aerospace]">
    <option value="Enterprise Super Carrier [aerospace]">
    <option value="Escape Pod [aerospace]">
    <option value="Essex I Destroyer [aerospace]">
    <option value="Essex II Destroyer [aerospace]">
    <option value="Excalibur [aerospace]">
    <option value="Explorer JumpShip [aerospace]">
    <option value="Factory [aerospace]">
    <option value="Farragut Battleship [aerospace]">
    <option value="Faslane Yardship [aerospace]">
    <option value="Feng Huang Cruiser [aerospace]">
    <option value="Firebird [aerospace]">
    <option value="Fortress [aerospace]">
    <option value="Fox Corvette [aerospace]">
    <option value="Foxhound Gunboat [aerospace]">
    <option value="Fredasa (Corvette-Raider) [aerospace]">
    <option value="Fury [aerospace]">
    <option value="Gaajian System Patrol Boat [aerospace]">
    <option value="Gazelle [aerospace]">
    <option value="Gorgon Carrier [aerospace]">
    <option value="Goth [aerospace]">
    <option value="Gotha [aerospace]">
    <option value="Graceful Crane Air Bus [aerospace]">
    <option value="Guardian Fighter [aerospace]">
    <option value="Habitat [aerospace]">
    <option value="Hamilcar [aerospace]">
    <option value="Hammerhead [aerospace]">
    <option value="Hannibal [aerospace]">
    <option value="Heavy Strike Fighter [aerospace]">
    <option value="Hellcat [aerospace]">
    <option value="Hellcat II [aerospace]">
    <option value="Hercules [aerospace]">
    <option value="Hong Lung Interdiction Station [aerospace]">
    <option value="Hoshiryokou [aerospace]">
    <option value="Hunter JumpShip [aerospace]">
    <option value="Hurricane Conventional Fighter [aerospace]">
    <option value="Huscarl [aerospace]">
    <option value="Hydaspes [aerospace]">
    <option value="Impavido Destroyer [aerospace]">
    <option value="Inazuma Corvette [aerospace]">
    <option value="Interdictor Pocket WarShip [aerospace]">
    <option value="Intrepid Assault Craft [aerospace]">
    <option value="Intruder [aerospace]">
    <option value="Invader JumpShip [aerospace]">
    <option value="Ironsides [aerospace]">
    <option value="Isegrim Assault DropShip [aerospace]">
    <option value="Issedone [aerospace]">
    <option value="Issus [aerospace]">
    <option value="Jagatai [aerospace]">
    <option value="Jengiz [aerospace]">
    <option value="Jetta Coruna 4X [aerospace]">
    <option value="Jumbo [aerospace]">
    <option value="Katya Ground Assault Craft [aerospace]">
    <option value="Kimagure Pursuit Cruiser [aerospace]">
    <option value="King Karnov Transport [aerospace]">
    <option value="Kirghiz [aerospace]">
    <option value="Kirishima Cruiser [aerospace]">
    <option value="Koroshiya [aerospace]">
    <option value="Kuan Ti [aerospace]">
    <option value="Kublai [aerospace]">
    <option value="Kyushu Frigate [aerospace]">
    <option value="Lancer [aerospace]">
    <option value="League Destroyer [aerospace]">
    <option value="Lee [aerospace]">
    <option value="Leopard [aerospace]">
    <option value="Leopard CV [aerospace]">
    <option value="Leviathan Heavy Transport [aerospace]">
    <option value="Leviathan II Battleship [aerospace]">
    <option value="Leviathan III Battleship [aerospace]">
    <option value="Leviathan JumpShip [aerospace]">
    <option value="Liberator Cruiser [aerospace]">
    <option value="Liberty JumpShip [aerospace]">
    <option value="Life Boat [aerospace]">
    <option value="Light Strike Fighter [aerospace]">
    <option value="Lightning [aerospace]">
    <option value="Lion [aerospace]">
    <option value="Lola I Destroyer [aerospace]">
    <option value="Lola II Destroyer [aerospace]">
    <option value="Lola III Destroyer [aerospace]">
    <option value="Long-Range Shuttlecraft [aerospace]">
    <option value="Longhaul Cargo Aircraft [aerospace]">
    <option value="Lucifer [aerospace]">
    <option value="Lucifer II [aerospace]">
    <option value="Lucifer III [aerospace]">
    <option value="Lung Wang [aerospace]">
    <option value="Luxor Heavy Cruiser [aerospace]">
    <option value="Lyonesse Escort [aerospace]">
    <option value="M-9 SDS Battle Station [aerospace]">
    <option value="Magellan JumpShip [aerospace]">
    <option value="Mako Corvette [aerospace]">
    <option value="Malaika [aerospace]">
    <option value="Mammoth [aerospace]">
    <option value="Manatee [aerospace]">
    <option value="McKenna Battleship [aerospace]">
    <option value="Mechbuster [aerospace]">
    <option value="Medium Strike Fighter [aerospace]">
    <option value="Mercer [aerospace]">
    <option value="Merchant JumpShip [aerospace]">
    <option value="Merlin [aerospace]">
    <option value="Mingo [aerospace]">
    <option value="Miraborg [aerospace]">
    <option value="Mjolnir Battlecruiser [aerospace]">
    <option value="Model 96 &#x27;Elephant&#x27; [aerospace]">
    <option value="Model 96C [aerospace]">
    <option value="Model 97 &#x27;Octopus&#x27; [aerospace]">
    <option value="Molniya Corvette [aerospace]">
    <option value="Monarch [aerospace]">
    <option value="Monolith JumpShip [aerospace]">
    <option value="Monsoon Battleship [aerospace]">
    <option value="Monsoon Battleship (LF) [aerospace]">
    <option value="Morgenstern [aerospace]">
    <option value="Mosquito Radar Plane [aerospace]">
    <option value="Mule [aerospace]">
    <option value="Mule C [aerospace]">
    <option value="Mustang Fighter [aerospace]">
    <option value="Mówáng-class Courier [aerospace]">
    <option value="Mĕngqín [aerospace]">
    <option value="Naga Destroyer [aerospace]">
    <option value="Nagasawa [aerospace]">
    <option value="Nagumo [aerospace]">
    <option value="Narukami Destroyer [aerospace]">
    <option value="Nekohono&#x27;o [aerospace]">
    <option value="New Syrtis Carrier [aerospace]">
    <option value="Newgrange III Yardship [aerospace]">
    <option value="Nightlord Battleship [aerospace]">
    <option value="Nightwing Surveillance [aerospace]">
    <option value="NL-45 Gunboat [aerospace]">
    <option value="Noruff [aerospace]">
    <option value="Odyssey JumpShip [aerospace]">
    <option value="Ogotai [aerospace]">
    <option value="Okinawa [aerospace]">
    <option value="Olympus Recharge Station [aerospace]">
    <option value="Oni [aerospace]">
    <option value="Oo-Suzumebachi [aerospace]">
    <option value="Ostrogoth [aerospace]">
    <option value="Outpost [aerospace]">
    <option value="Overlord [aerospace]">
    <option value="Overlord C [aerospace]">
    <option value="Pella [aerospace]">
    <option value="Pentagon [aerospace]">
    <option value="Peregrine Corvette [aerospace]">
    <option value="Persepolis [aerospace]">
    <option value="Picaroon [aerospace]">
    <option value="Pinto Corvette [aerospace]">
    <option value="Planetlifter Air Transport [aerospace]">
    <option value="Planetlifter Support Aircraft [aerospace]">
    <option value="Planetlifter Tactical Support [aerospace]">
    <option value="Poignard [aerospace]">
    <option value="Polaris [aerospace]">
    <option value="Potemkin Troop Cruiser [aerospace]">
    <option value="Pressurized Yard [aerospace]">
    <option value="Princess Luxury Liner [aerospace]">
    <option value="Protector Combat Support Fighter [aerospace]">
    <option value="Protector High-Speed Medevac [aerospace]">
    <option value="Pueblo [aerospace]">
    <option value="Qasar [aerospace]">
    <option value="Quetzalcoatl-Scout JumpShip [aerospace]">
    <option value="Quicksilver Mongoose Battleship [aerospace]">
    <option value="Quixote Frigate [aerospace]">
    <option value="Rapier [aerospace]">
    <option value="Raubvogel Aerobomber [aerospace]">
    <option value="Riever [aerospace]">
    <option value="Riga Frigate [aerospace]">
    <option value="Riga II Destroyer-Carrier [aerospace]">
    <option value="Robinson Transport [aerospace]">
    <option value="Rogue [aerospace]">
    <option value="Rondel [aerospace]">
    <option value="Rose (Bara no Ryu) [aerospace]">
    <option value="Rusalka [aerospace]">
    <option value="S 2772 Airplane [aerospace]">
    <option value="S7-P Bus [aerospace]">
    <option value="Sabre [aerospace]">
    <option value="Sabutai [aerospace]">
    <option value="Sagittarii [aerospace]">
    <option value="Sai [aerospace]">
    <option value="Samarkand Carrier [aerospace]">
    <option value="Samurai [aerospace]">
    <option value="Sangihe Shrike-Thrush &#x27;Nanook&#x27; [aerospace]">
    <option value="Saroyan Jump Bomber [aerospace]">
    <option value="Sassanid [aerospace]">
    <option value="Saturn Patrol Ship [aerospace]">
    <option value="Scarab Medium Drone Fighter [aerospace]">
    <option value="Schrack [aerospace]">
    <option value="Scout JumpShip [aerospace]">
    <option value="Scytha [aerospace]">
    <option value="Seabuster Strike Fighter [aerospace]">
    <option value="Seeker [aerospace]">
    <option value="Seleucus Infantry Transport [aerospace]">
    <option value="Seydlitz [aerospace]">
    <option value="Shade [aerospace]">
    <option value="Shikra [aerospace]">
    <option value="Shilone [aerospace]">
    <option value="Shiva [aerospace]">
    <option value="Sholagar [aerospace]">
    <option value="Shuttle [aerospace]">
    <option value="Simurgh [aerospace]">
    <option value="Slayer [aerospace]">
    <option value="Snowden Mining Station [aerospace]">
    <option value="Sovetskii Soyuz Heavy Cruiser [aerospace]">
    <option value="Sovetskii Soyuz Heavy Cruiser (Dire Wolf) [aerospace]">
    <option value="Soyal Heavy Cruiser [aerospace]">
    <option value="Spad [aerospace]">
    <option value="Sparrowhawk [aerospace]">
    <option value="Star Dagger [aerospace]">
    <option value="Star Lord JumpShip [aerospace]">
    <option value="Starfire [aerospace]">
    <option value="Stefan Amaris Battleship [aerospace]">
    <option value="Sternensturm [aerospace]">
    <option value="Stingray [aerospace]">
    <option value="Stork Light Refueling Craft [aerospace]">
    <option value="Stratos Airliner [aerospace]">
    <option value="Striga [aerospace]">
    <option value="Stuka [aerospace]">
    <option value="Suffren Destroyer [aerospace]">
    <option value="Sulla [aerospace]">
    <option value="Suzaku [aerospace]">
    <option value="Swift [aerospace]">
    <option value="Sylvester Transport [aerospace]">
    <option value="Tabanid Light Drone Fighter [aerospace]">
    <option value="Taihou Assault DropShip [aerospace]">
    <option value="Tatsu [aerospace]">
    <option value="Tatsumaki Destroyer [aerospace]">
    <option value="Texas Battleship [aerospace]">
    <option value="Tharkad Battlecruiser [aerospace]">
    <option value="Thera Carrier [aerospace]">
    <option value="Thrush [aerospace]">
    <option value="Thunderbird [aerospace]">
    <option value="Tiamat II [aerospace]">
    <option value="Tiamat Pocket WarShip [aerospace]">
    <option value="Tigress Close Patrol Craft [aerospace]">
    <option value="Titan [aerospace]">
    <option value="Tomahawk [aerospace]">
    <option value="Torrent Heavy Bomber [aerospace]">
    <option value="Tracker Surveillance [aerospace]">
    <option value="Tramp JumpShip [aerospace]">
    <option value="Transgressor [aerospace]">
    <option value="Transit [aerospace]">
    <option value="Trident [aerospace]">
    <option value="Triumph [aerospace]">
    <option value="Troika [aerospace]">
    <option value="Trojan [aerospace]">
    <option value="Trutzburg [aerospace]">
    <option value="Tsuru VIP Aircraft [aerospace]">
    <option value="Turk [aerospace]">
    <option value="Typhoon [aerospace]">
    <option value="Tyre [aerospace]">
    <option value="Umbra [aerospace]">
    <option value="Union [aerospace]">
    <option value="Union C [aerospace]">
    <option value="Union-X [aerospace]">
    <option value="Unpressurized Yard [aerospace]">
    <option value="Vampire [aerospace]">
    <option value="Vandal [aerospace]">
    <option value="Vanir Assault DropShip [aerospace]">
    <option value="Vendetta Medium Fighter [aerospace]">
    <option value="Vengeance [aerospace]">
    <option value="Vigilant Corvette [aerospace]">
    <option value="Vincent Corvette [aerospace]">
    <option value="Visigoth [aerospace]">
    <option value="Voidseeker [aerospace]">
    <option value="Volga Transport [aerospace]">
    <option value="Vulcan [aerospace]">
    <option value="Vulture [aerospace]">
    <option value="Wagon Wheel Frigate [aerospace]">
    <option value="Wheeler-Class Station [aerospace]">
    <option value="Whirlwind Destroyer [aerospace]">
    <option value="Wildkatze [aerospace]">
    <option value="Winchester Cruiser [aerospace]">
    <option value="Wusun [aerospace]">
    <option value="Würger Assault Craft [aerospace]">
    <option value="Xerxes [aerospace]">
    <option value="York Destroyer-Carrier [aerospace]">
    <option value="Yùn [aerospace]">
    <option value="Zanadu Air Bus [aerospace]">
    <option value="Zechetinu Corvette [aerospace]">
    <option value="Zechetinu II Corvette [aerospace]">
    <option value="Zero [aerospace]">
    <option value="Zhen Niao [aerospace]">
    <option value="AA Jump Infantry [infantry]">
    <option value="AA Mechanized Infantry [infantry]">
    <option value="Anti-&#x27;Mech Jump Infantry [infantry]">
    <option value="Anti-Infantry Unit [infantry]">
    <option value="Assault Commando [infantry]">
    <option value="Bandit Motorized Point [infantry]">
    <option value="Beast Infantry [infantry]">
    <option value="Beast Infantry (Branth) [infantry]">
    <option value="Beast Infantry (Camel) [infantry]">
    <option value="Beast Infantry (Donkey) [infantry]">
    <option value="Beast Infantry (Elephant) [infantry]">
    <option value="Beast Infantry (Hipposaur) [infantry]">
    <option value="Beast Infantry (Horse) [infantry]">
    <option value="Beast Infantry (Kangaroo) [infantry]">
    <option value="Beast Infantry (Odessan Raxx) [infantry]">
    <option value="Beast Infantry (Orca) [infantry]">
    <option value="Beast Infantry (Tabiranth) [infantry]">
    <option value="Beast Infantry (Tariq) [infantry]">
    <option value="Bridge-builder Engineers [infantry]">
    <option value="Ceremonial Guard [infantry]">
    <option value="Ceremonial Platoon [infantry]">
    <option value="Clan Anti-Infantry [infantry]">
    <option value="Clan Assault Infantry [infantry]">
    <option value="Clan Field Artillery [infantry]">
    <option value="Clan Field Artillery Point [infantry]">
    <option value="Clan Field Gun Point [infantry]">
    <option value="Clan Field Gunners [infantry]">
    <option value="Clan Foot Infantry [infantry]">
    <option value="Clan Foot Point [infantry]">
    <option value="Clan Foot Point (Anti-&#x27;Mech) [infantry]">
    <option value="Clan Foot Squad [infantry]">
    <option value="Clan Foot Squad (Anti-&#x27;Mech) [infantry]">
    <option value="Clan Heavy Foot Infantry [infantry]">
    <option value="Clan Heavy Jump Infantry [infantry]">
    <option value="Clan Jump Point [infantry]">
    <option value="Clan Jump Squad [infantry]">
    <option value="Clan Mechanized Hover Point [infantry]">
    <option value="Clan Mechanized Hover Squad [infantry]">
    <option value="Clan Mechanized Infantry [infantry]">
    <option value="Clan Mechanized Tracked Point [infantry]">
    <option value="Clan Mechanized Tracked Squad [infantry]">
    <option value="Clan Mechanized Wheeled Point [infantry]">
    <option value="Clan Mechanized Wheeled Squad [infantry]">
    <option value="Clan Motorized Point [infantry]">
    <option value="Clan Motorized Squad [infantry]">
    <option value="Clan Space Marine [infantry]">
    <option value="Combat Engineer [infantry]">
    <option value="Commando [infantry]">
    <option value="Fast Recon [infantry]">
    <option value="Field Artillery [infantry]">
    <option value="Field Artillery Century (MHAF) [infantry]">
    <option value="Field Artillery demi-I (ComGuards) [infantry]">
    <option value="Field Artillery demi-I (WOBM) [infantry]">
    <option value="Field Artillery Level I (ComGuards) [infantry]">
    <option value="Field Artillery Level I (WOBM) [infantry]">
    <option value="Field Artillery Platoon (AFFS) [infantry]">
    <option value="Field Artillery Platoon (CCAF) [infantry]">
    <option value="Field Artillery Platoon (ComStar) [infantry]">
    <option value="Field Artillery Platoon (DCMS) [infantry]">
    <option value="Field Artillery Platoon (FWLM) [infantry]">
    <option value="Field Artillery Platoon (LCAF) [infantry]">
    <option value="Field Gun Century (MHAF) [infantry]">
    <option value="Field Gun demi-I (ComGuards) [infantry]">
    <option value="Field Gun demi-I (WOBM) [infantry]">
    <option value="Field Gun Infantry [infantry]">
    <option value="Field Gun Infantry Platoon (AC/10) [infantry]">
    <option value="Field Gun Level I (ComGuards) [infantry]">
    <option value="Field Gun Level I (WOBM) [infantry]">
    <option value="Field Gun Platoon (AFFS) [infantry]">
    <option value="Field Gun Platoon (CCAF) [infantry]">
    <option value="Field Gun Platoon (ComStar) [infantry]">
    <option value="Field Gun Platoon (DCMS) [infantry]">
    <option value="Field Gun Platoon (FWLM) [infantry]">
    <option value="Field Gun Platoon (LCAF) [infantry]">
    <option value="Field Gunners [infantry]">
    <option value="Field Medic [infantry]">
    <option value="Firefighter [infantry]">
    <option value="Foot Ballistic Rifle [infantry]">
    <option value="Foot Contubernium (MHAF) [infantry]">
    <option value="Foot demi-I (ComGuards) [infantry]">
    <option value="Foot demi-I (Domini) [infantry]">
    <option value="Foot demi-I (WOBM) [infantry]">
    <option value="Foot Duplus Contubernium (MHAF) [infantry]">
    <option value="Foot Infantry [infantry]">
    <option value="Foot Platoon [infantry]">
    <option value="Foot Platoon (AFFS) [infantry]">
    <option value="Foot Platoon (Anti-&#x27;Mech) [infantry]">
    <option value="Foot Platoon (CCAF) [infantry]">
    <option value="Foot Platoon (ComStar) [infantry]">
    <option value="Foot Platoon (DCMS) [infantry]">
    <option value="Foot Platoon (FWLM) [infantry]">
    <option value="Foot Platoon (LCAF) [infantry]">
    <option value="Foot Platoon (Taurian 3047+) [infantry]">
    <option value="Foot Platoon (Taurian) [infantry]">
    <option value="Foot Squad [infantry]">
    <option value="Foot Squad (Anti-&#x27;Mech) [infantry]">
    <option value="Foot Stealth Platoon [infantry]">
    <option value="Foot Stealth Squad [infantry]">
    <option value="Foot Triplus Contubernium (MHAF) [infantry]">
    <option value="Frogmen [infantry]">
    <option value="HALO Paratrooper [infantry]">
    <option value="Heavy Foot LRM Infantry [infantry]">
    <option value="Heavy Infantry [infantry]">
    <option value="Heavy Jump Infantry [infantry]">
    <option value="Heavy Mountain Infantry [infantry]">
    <option value="Heavy Support Infantry [infantry]">
    <option value="Hover Assault Infantry [infantry]">
    <option value="Jump Contubernium (MHAF) [infantry]">
    <option value="Jump Duplus Contubernium (MHAF) [infantry]">
    <option value="Jump Laser Infantry [infantry]">
    <option value="Jump Level I (ComGuards) [infantry]">
    <option value="Jump Level I (Domini) [infantry]">
    <option value="Jump Level I (WOBM) [infantry]">
    <option value="Jump Platoon [infantry]">
    <option value="Jump Platoon (AFFS) [infantry]">
    <option value="Jump Platoon (Anti-Mech) [infantry]">
    <option value="Jump Platoon (CCAF) [infantry]">
    <option value="Jump Platoon (ComStar) [infantry]">
    <option value="Jump Platoon (DCMS) [infantry]">
    <option value="Jump Platoon (FWLM) [infantry]">
    <option value="Jump Platoon (LCAF) [infantry]">
    <option value="Jump Platoon (Taurian 3047+) [infantry]">
    <option value="Jump Platoon (Taurian) [infantry]">
    <option value="Jump Squad [infantry]">
    <option value="Jump Stealth Platoon [infantry]">
    <option value="Jump Support Infantry [infantry]">
    <option value="Jump Triplus Contubernium (MHAF) [infantry]">
    <option value="Manei Domini Attack Squad [infantry]">
    <option value="Manei Domini Recon Squad [infantry]">
    <option value="Mechanized Assault XCT [infantry]">
    <option value="Mechanized Field Artillery [infantry]">
    <option value="Mechanized Hover Century (MHAF) [infantry]">
    <option value="Mechanized Hover Level I (ComGuards) [infantry]">
    <option value="Mechanized Hover Level I (WOBM) [infantry]">
    <option value="Mechanized Hover Platoon [infantry]">
    <option value="Mechanized Hover Platoon (AFFS) [infantry]">
    <option value="Mechanized Hover Platoon (CCAF) [infantry]">
    <option value="Mechanized Hover Platoon (ComStar) [infantry]">
    <option value="Mechanized Hover Platoon (DCMS) [infantry]">
    <option value="Mechanized Hover Platoon (FWLM) [infantry]">
    <option value="Mechanized Hover Platoon (LCAF) [infantry]">
    <option value="Mechanized Hover Platoon (Taurian 3047+) [infantry]">
    <option value="Mechanized Hover Platoon (Taurian) [infantry]">
    <option value="Mechanized Hover Squad [infantry]">
    <option value="Mechanized Sub Platoon [infantry]">
    <option value="Mechanized Tracked Century (MHAF) [infantry]">
    <option value="Mechanized Tracked Level I (ComGuards) [infantry]">
    <option value="Mechanized Tracked Level I (WOBM) [infantry]">
    <option value="Mechanized Tracked Platoon [infantry]">
    <option value="Mechanized Tracked Platoon (AFFS) [infantry]">
    <option value="Mechanized Tracked Platoon (CCAF) [infantry]">
    <option value="Mechanized Tracked Platoon (ComStar) [infantry]">
    <option value="Mechanized Tracked Platoon (DCMS) [infantry]">
    <option value="Mechanized Tracked Platoon (FWLM) [infantry]">
    <option value="Mechanized Tracked Platoon (LCAF) [infantry]">
    <option value="Mechanized Tracked Platoon (Taurian 3047+) [infantry]">
    <option value="Mechanized Tracked Platoon (Taurian) [infantry]">
    <option value="Mechanized Tracked Squad [infantry]">
    <option value="Mechanized Wheeled Century (MHAF) [infantry]">
    <option value="Mechanized Wheeled Level I (ComGuards) [infantry]">
    <option value="Mechanized Wheeled Level I (WOBM) [infantry]">
    <option value="Mechanized Wheeled Platoon [infantry]">
    <option value="Mechanized Wheeled Platoon (AFFS) [infantry]">
    <option value="Mechanized Wheeled Platoon (CCAF) [infantry]">
    <option value="Mechanized Wheeled Platoon (ComStar) [infantry]">
    <option value="Mechanized Wheeled Platoon (DCMS) [infantry]">
    <option value="Mechanized Wheeled Platoon (FWLM) [infantry]">
    <option value="Mechanized Wheeled Platoon (LCAF) [infantry]">
    <option value="Mechanized Wheeled Platoon (Taurian 3047+) [infantry]">
    <option value="Mechanized Wheeled Platoon (Taurian) [infantry]">
    <option value="Mechanized Wheeled Squad [infantry]">
    <option value="Minesweepers [infantry]">
    <option value="Missile Artillery Infantry [infantry]">
    <option value="Mob [infantry]">
    <option value="Motorized demi-I (ComGuards) [infantry]">
    <option value="Motorized demi-I (Domini) [infantry]">
    <option value="Motorized demi-I (WOBM) [infantry]">
    <option value="Motorized Duplus Contubernium (MHAF) [infantry]">
    <option value="Motorized Heavy Infantry [infantry]">
    <option value="Motorized Infantry [infantry]">
    <option value="Motorized MG [infantry]">
    <option value="Motorized Platoon [infantry]">
    <option value="Motorized Platoon (AFFS) [infantry]">
    <option value="Motorized Platoon (CCAF) [infantry]">
    <option value="Motorized Platoon (ComStar) [infantry]">
    <option value="Motorized Platoon (DCMS) [infantry]">
    <option value="Motorized Platoon (FWLM) [infantry]">
    <option value="Motorized Platoon (LCAF) [infantry]">
    <option value="Motorized Platoon (Taurian 3047+) [infantry]">
    <option value="Motorized Platoon (Taurian) [infantry]">
    <option value="Motorized Squad [infantry]">
    <option value="Motorized Sub Platoon [infantry]">
    <option value="Motorized Triplus Contubernium (MHAF) [infantry]">
    <option value="Motorized XCT Infantry [infantry]">
    <option value="Mountaineer [infantry]">
    <option value="Pirate [infantry]">
    <option value="Recon Infantry [infantry]">
    <option value="Riot Police [infantry]">
    <option value="Scout Infantry [infantry]">
    <option value="Skaret Assassins [infantry]">
    <option value="Sniper [infantry]">
    <option value="Space Marine [infantry]">
    <option value="Special Forces [infantry]">
    <option value="SpecOps Paratrooper [infantry]">
    <option value="SRM Foot Infantry [infantry]">
    <option value="Submersible Mechanized Infantry [infantry]">
    <option value="Surveillance Specialist [infantry]">
    <option value="TAG Spotter Infantry [infantry]">
    <option value="VTOL Infantry [infantry]">
    <option value="XCT Marine [infantry]">
    <option value="Xenoplanetary Infantry [infantry]">
    <option value="Achileus Light Battle Armor [battlearmor]">
    <option value="Aegis Point Defense Suit [battlearmor]">
    <option value="Aerie PA(L) [battlearmor]">
    <option value="Afreet Medium Battle Armor [battlearmor]">
    <option value="Ailette Rescue PA(L) [battlearmor]">
    <option value="Ailette Zero-G Engineering Exoskeleton [battlearmor]">
    <option value="Amazon Battle Armor [battlearmor]">
    <option value="Angerona Scout Suit [battlearmor]">
    <option value="Asura Medium Battle Armor [battlearmor]">
    <option value="Black Wolf Battle Armor [battlearmor]">
    <option value="Buraq Fast Battle Armor [battlearmor]">
    <option value="Callisto Battle Armor [battlearmor]">
    <option value="Cavalier Battle Armor [battlearmor]">
    <option value="Cavalier II Battle Armor [battlearmor]">
    <option value="Centaur Battle Armor [battlearmor]">
    <option value="Clan Interface Armor [battlearmor]">
    <option value="Clan Medium Battle Armor [battlearmor]">
    <option value="Constable Pacification Suit [battlearmor]">
    <option value="Corona Heavy Battle Armor [battlearmor]">
    <option value="Cuchulainn Support Armor [battlearmor]">
    <option value="Djinn Battle Armor [battlearmor]">
    <option value="Dragoon Battle Armor [battlearmor]">
    <option value="Elemental Battle Armor [battlearmor]">
    <option value="Elemental II Battle Armor [battlearmor]">
    <option value="Elemental III Battle Armor [battlearmor]">
    <option value="Fa Shih Battle Armor [battlearmor]">
    <option value="Fenrir Battle Armor [battlearmor]">
    <option value="Fenrir II Assault Battle Armor [battlearmor]">
    <option value="Fusilier Battle Armor [battlearmor]">
    <option value="Gladiator Battle Armor [battlearmor]">
    <option value="Gladiator Exoskeleton [battlearmor]">
    <option value="Gnome Battle Armor [battlearmor]">
    <option value="Golem Assault Armor [battlearmor]">
    <option value="Gorilla Exoskeleton [battlearmor]">
    <option value="Gray Death Heavy Suit [battlearmor]">
    <option value="Gray Death Infiltrator Suit [battlearmor]">
    <option value="Gray Death Scout Suit [battlearmor]">
    <option value="Gray Death Standard Suit [battlearmor]">
    <option value="Gray Death Strike Suit [battlearmor]">
    <option value="Grenadier Battle Armor [battlearmor]">
    <option value="Grenadier II Battle Armor [battlearmor]">
    <option value="Groundhog Exoskeleton [battlearmor]">
    <option value="Hantu [battlearmor]">
    <option value="Hauberk Battle Armor [battlearmor]">
    <option value="Hauberk II Battle Armor [battlearmor]">
    <option value="HeavyHauler Exoskeleton [battlearmor]">
    <option value="Infiltrator Mk. I Battle Armor [battlearmor]">
    <option value="Infiltrator Mk. II Battle Armor [battlearmor]">
    <option value="Ironhold Assault Battle Armor [battlearmor]">
    <option value="IS Standard Battle Armor [battlearmor]">
    <option value="Kage Light Battle Armor [battlearmor]">
    <option value="Kanazuchi Assault Battle Armor [battlearmor]">
    <option value="Kishi Ceremonial Armor [battlearmor]">
    <option value="Kobold Battle Armor [battlearmor]">
    <option value="Kobold Battle Armor IIC [battlearmor]">
    <option value="Kopis Assault Battle Armor [battlearmor]">
    <option value="Krise PA(L) [battlearmor]">
    <option value="Leonidas Battle Armor [battlearmor]">
    <option value="Longinus Battle Armor [battlearmor]">
    <option value="Longinus C Battle Armor [battlearmor]">
    <option value="Machina Domini Interface Armor [battlearmor]">
    <option value="Marauder Battle Armor [battlearmor]">
    <option value="Nephilim Assault Battle Armor [battlearmor]">
    <option value="Nighthawk PA(L) [battlearmor]">
    <option value="Ogre Battle Armor [battlearmor]">
    <option value="Oni Battle Armor [battlearmor]">
    <option value="PAB-28 Sniper Suit [battlearmor]">
    <option value="Phalanx Battle Armor [battlearmor]">
    <option value="PowerLoader Exoskeleton [battlearmor]">
    <option value="Purifier Adaptive Battle Armor [battlearmor]">
    <option value="Purifier Battle Armor Terra [battlearmor]">
    <option value="Quirinus Battle Armor [battlearmor]">
    <option value="Raiden Battle Armor [battlearmor]">
    <option value="Raiden II Battle Armor [battlearmor]">
    <option value="Ravager Assault Battle Armor [battlearmor]">
    <option value="Resgate PA(L) [battlearmor]">
    <option value="Rhino Battle Armor [battlearmor]">
    <option value="Rogue Bear Heavy Battle Armor [battlearmor]">
    <option value="Rottweiler Battle Armor [battlearmor]">
    <option value="Salamander Battle Armor [battlearmor]">
    <option value="Salrilla Exoskeleton [battlearmor]">
    <option value="Se&#x27;irim Medium Battle Armor [battlearmor]">
    <option value="Sea Fox Amphibious Armor [battlearmor]">
    <option value="Shedu Assault Battle Armor [battlearmor]">
    <option value="Shen Long Battle Armor [battlearmor]">
    <option value="Simian Battle Armor [battlearmor]">
    <option value="Sloth Battle Armor [battlearmor]">
    <option value="Smoothdavid PA(L) [battlearmor]">
    <option value="Smoothgoliath PA(L) [battlearmor]">
    <option value="Spectre Stealth Battle Armor [battlearmor]">
    <option value="Stormbird Battle Armor [battlearmor]">
    <option value="Surat (Gray Death) Solahma Suit [battlearmor]">
    <option value="Sylph Battle Armor [battlearmor]">
    <option value="Taranis Battle Armor [battlearmor]">
    <option value="Tengu Heavy Battle Armor [battlearmor]">
    <option value="Thunderbird Battle Armor [battlearmor]">
    <option value="Thunderbird II Battle Armor [battlearmor]">
    <option value="TinStar BattleArmor [battlearmor]">
    <option value="Tornado PA(L) [battlearmor]">
    <option value="Tortoise II [battlearmor]">
    <option value="Trinity Medium Battle Armor [battlearmor]">
    <option value="Tunnel Rat I Mining Exoskeleton [battlearmor]">
    <option value="Tunnel Rat II Mining Exoskeleton [battlearmor]">
    <option value="Tunnel Rat III Mining Exoskeleton [battlearmor]">
    <option value="Tunnel Rat IV Mining Exoskeleton [battlearmor]">
    <option value="Undine Battle Armor [battlearmor]">
    <option value="Void Medium Battle Armor [battlearmor]">
    <option value="Warg Assault Battle Armor [battlearmor]">
    <option value="Water Elemental Mining Suit [battlearmor]">
    <option value="Wraith Battle Armor [battlearmor]">
    <option value="Xiphos Assault Battle Armor [battlearmor]">
    <option value="Zou Heavy Battle Armor [battlearmor]">
    <option value="&#x27;Mech Mortar/1 Battery [emplacement]">
    <option value="&#x27;Mech Mortar/2 Battery [emplacement]">
    <option value="&#x27;Mech Mortar/4 Battery [emplacement]">
    <option value="&#x27;Mech Mortar/8 Battery [emplacement]">
    <option value="AC/10 Turret [emplacement]">
    <option value="AC/2 Turret [emplacement]">
    <option value="AC/20 Turret [emplacement]">
    <option value="AC/5 Turret [emplacement]">
    <option value="Active Probe Bunker [emplacement]">
    <option value="AMS Turret [emplacement]">
    <option value="Angel ECM Bunker [emplacement]">
    <option value="AP Gauss Turret [emplacement]">
    <option value="Arrow IV Artillery Turret [emplacement]">
    <option value="Assault Air Defense Missile Emplacement [emplacement]">
    <option value="Assault Bombard Turret [emplacement]">
    <option value="Assault Flak Turret [emplacement]">
    <option value="Assault Laser Turret [emplacement]">
    <option value="Assault Missile Turret [emplacement]">
    <option value="Assault Shredder Turret [emplacement]">
    <option value="Assault Siege Turret [emplacement]">
    <option value="Assault Sniper Turret [emplacement]">
    <option value="ATM/12 Turret [emplacement]">
    <option value="ATM/3 Turret [emplacement]">
    <option value="ATM/6 Turret [emplacement]">
    <option value="ATM/9 Turret [emplacement]">
    <option value="Beagle Active Probe Bunker [emplacement]">
    <option value="Blazer Cannon Turret [emplacement]">
    <option value="Bloodhound Active Probe Bunker [emplacement]">
    <option value="Bombast Laser Turret [emplacement]">
    <option value="C3 (Boosted) Command Bunker [emplacement]">
    <option value="C3 Command Bunker [emplacement]">
    <option value="C3i Bunker [emplacement]">
    <option value="Calliope Turret [emplacement]">
    <option value="Capellan Advanced Shredder Turret [emplacement]">
    <option value="Capellan Advanced Siege Turret [emplacement]">
    <option value="Cruise Missile/120 Launcher [emplacement]">
    <option value="Cruise Missile/50 Launcher [emplacement]">
    <option value="Cruise Missile/70 Launcher [emplacement]">
    <option value="Cruise Missile/90 Launcher [emplacement]">
    <option value="Draconis Advanced Missile Turret [emplacement]">
    <option value="Draconis Advanced Siege Turret [emplacement]">
    <option value="ECM Suite Bunker [emplacement]">
    <option value="ECM Turret (Guardian) [emplacement]">
    <option value="Enhanced LRM/10 Turret [emplacement]">
    <option value="Enhanced LRM/15 Turret [emplacement]">
    <option value="Enhanced LRM/20 Turret [emplacement]">
    <option value="Enhanced LRM/5 Turret [emplacement]">
    <option value="Enhanced PPC Turret [emplacement]">
    <option value="ER Flamer Turret [emplacement]">
    <option value="ER Large Laser Turret [emplacement]">
    <option value="ER Large Laser Turret (w/ Targeting) [emplacement]">
    <option value="ER Medium Laser Turret [emplacement]">
    <option value="ER Micro Laser Turret [emplacement]">
    <option value="ER PPC Turret [emplacement]">
    <option value="ER PPC w Capacitor Turret [emplacement]">
    <option value="ER Small Laser Turret [emplacement]">
    <option value="Extended LRM/10 Turret [emplacement]">
    <option value="Extended LRM/15 Turret [emplacement]">
    <option value="Extended LRM/20 Turret [emplacement]">
    <option value="Extended LRM/5 Turret [emplacement]">
    <option value="Fed Suns Advanced Laser Turret [emplacement]">
    <option value="Fed Suns Advanced Shredder Turret [emplacement]">
    <option value="Flamer Turret [emplacement]">
    <option value="FWL Advanced Laser Turret [emplacement]">
    <option value="FWL Advanced Missile Turret [emplacement]">
    <option value="Gauss Turret [emplacement]">
    <option value="Grenade Launcher Turret [emplacement]">
    <option value="HAG/20 Turret [emplacement]">
    <option value="HAG/30 Turret [emplacement]">
    <option value="HAG/40 Turret [emplacement]">
    <option value="Heavy Blaze Turret [emplacement]">
    <option value="Heavy Bombard Turret [emplacement]">
    <option value="Heavy Flak Turret [emplacement]">
    <option value="Heavy Flamer Turret [emplacement]">
    <option value="Heavy Gauss Turret [emplacement]">
    <option value="Heavy Large Laser Turret [emplacement]">
    <option value="Heavy Large Laser Turret(w/ Targeting) [emplacement]">
    <option value="Heavy Laser Turret [emplacement]">
    <option value="Heavy Medium Laser Turret [emplacement]">
    <option value="Heavy MG Turret [emplacement]">
    <option value="Heavy MG Turret w/ Array [emplacement]">
    <option value="Heavy Missile Turret [emplacement]">
    <option value="Heavy PPC Turret [emplacement]">
    <option value="Heavy PPC w Capacitor Turret [emplacement]">
    <option value="Heavy Rifle Turret [emplacement]">
    <option value="Heavy Shredder Turret [emplacement]">
    <option value="Heavy Siege Turret [emplacement]">
    <option value="Heavy Small Laser Turret [emplacement]">
    <option value="Heavy Sniper Turret [emplacement]">
    <option value="HV Autocannon/10 Turret [emplacement]">
    <option value="HV Autocannon/2 Turret [emplacement]">
    <option value="HV Autocannon/5 Turret [emplacement]">
    <option value="Imp.Heavy Gauss Turret [emplacement]">
    <option value="Improved ATM/12 Turret [emplacement]">
    <option value="Improved ATM/3 Turret [emplacement]">
    <option value="Improved ATM/6 Turret [emplacement]">
    <option value="Improved ATM/9 Turret [emplacement]">
    <option value="Improved Heavy Large Laser Turret [emplacement]">
    <option value="Improved Heavy Large Laser Turret (w/ Targeting) [emplacement]">
    <option value="Improved Heavy Medium Laser Turret [emplacement]">
    <option value="Improved Heavy Small Laser Turret [emplacement]">
    <option value="Improved Narc Turret [emplacement]">
    <option value="Large Chemical Laser Turret [emplacement]">
    <option value="Large ER Pulse Laser Turret [emplacement]">
    <option value="Large Laser Turret [emplacement]">
    <option value="Large Pulse Laser Turret [emplacement]">
    <option value="Large Re-engineered Laser Turret [emplacement]">
    <option value="Large VSP Laser Turret [emplacement]">
    <option value="Large X-Pulse Laser Turret [emplacement]">
    <option value="Laser AMS Turret [emplacement]">
    <option value="LB-10-X Turret [emplacement]">
    <option value="LB-2-X Turret [emplacement]">
    <option value="LB-20-X Turret [emplacement]">
    <option value="LB-5-X Turret [emplacement]">
    <option value="Light AC/2 Turret [emplacement]">
    <option value="Light AC/5 Turret [emplacement]">
    <option value="Light Active Probe Bunker [emplacement]">
    <option value="Light Bombard Turret [emplacement]">
    <option value="Light Flak Turret [emplacement]">
    <option value="Light Gauss Turret [emplacement]">
    <option value="Light Laser Turret [emplacement]">
    <option value="Light MG Turret [emplacement]">
    <option value="Light MG Turret w/ Array [emplacement]">
    <option value="Light Missile Turret [emplacement]">
    <option value="Light PPC Turret [emplacement]">
    <option value="Light PPC w Capacitor Turret [emplacement]">
    <option value="Light Rifle Turret [emplacement]">
    <option value="Light Shredder Turret [emplacement]">
    <option value="Light Sniper Turret [emplacement]">
    <option value="Long Tom Artillery Cannon Turret [emplacement]">
    <option value="Long Tom Artillery Turret [emplacement]">
    <option value="LRM/10 Turret [emplacement]">
    <option value="LRM/10 Turret (w/ Artemis IV) [emplacement]">
    <option value="LRM/10 Turret (w/ Artemis V) [emplacement]">
    <option value="LRM/15 Turret [emplacement]">
    <option value="LRM/15 Turret (w/ Artemis IV) [emplacement]">
    <option value="LRM/15 Turret (w/ Artemis V) [emplacement]">
    <option value="LRM/20 Turret [emplacement]">
    <option value="LRM/20 Turret (w/ Artemis IV) [emplacement]">
    <option value="LRM/20 Turret (w/ Artemis V) [emplacement]">
    <option value="LRM/5 Turret [emplacement]">
    <option value="LRM/5 Turret (w/ Artemis IV) [emplacement]">
    <option value="LRM/5 Turret (w/ Artemis V) [emplacement]">
    <option value="Lyran Advanced Missile Turret [emplacement]">
    <option value="Lyran Advanced Shredder Turret [emplacement]">
    <option value="Magshot Gauss Turret [emplacement]">
    <option value="Medium Air Defense Missile Emplacement [emplacement]">
    <option value="Medium Blaze Turret [emplacement]">
    <option value="Medium Bombard Turret [emplacement]">
    <option value="Medium Chemical Laser Turret [emplacement]">
    <option value="Medium Command Tower [emplacement]">
    <option value="Medium ER Pulse Laser Turret [emplacement]">
    <option value="Medium Flak Turret [emplacement]">
    <option value="Medium Laser Turret [emplacement]">
    <option value="Medium Missile Turret [emplacement]">
    <option value="Medium Pulse Laser Turret [emplacement]">
    <option value="Medium Re-engineered Laser Turret Turret [emplacement]">
    <option value="Medium Rifle Turret [emplacement]">
    <option value="Medium Shredder Turret [emplacement]">
    <option value="Medium Siege Turret [emplacement]">
    <option value="Medium Sniper Turret [emplacement]">
    <option value="Medium VSP Laser Turret [emplacement]">
    <option value="Medium X-Pulse Laser Turret [emplacement]">
    <option value="MG Turret [emplacement]">
    <option value="MG Turret w/ Array [emplacement]">
    <option value="Micro Pulse Laser Turret [emplacement]">
    <option value="MML/3 Turret [emplacement]">
    <option value="MML/3 Turret (w/ Artemis IV) [emplacement]">
    <option value="MML/5 Turret [emplacement]">
    <option value="MML/5 Turret (w/ Artemis IV) [emplacement]">
    <option value="MML/7 Turret [emplacement]">
    <option value="MML/7 Turret (w/ Artemis IV) [emplacement]">
    <option value="MML/9 Turret [emplacement]">
    <option value="MML/9 Turret (w/ Artemis IV) [emplacement]">
    <option value="MRM 10 Turret [emplacement]">
    <option value="MRM 10 Turret w/ Apollo FCS [emplacement]">
    <option value="MRM 20 Turret [emplacement]">
    <option value="MRM 20 Turret w/ Apollo FCS [emplacement]">
    <option value="MRM 30 Turret [emplacement]">
    <option value="MRM 30 Turret w/ Apollo FCS [emplacement]">
    <option value="MRM 40 Turret [emplacement]">
    <option value="MRM 40 Turret w/ Apollo FCS [emplacement]">
    <option value="Narc Turret [emplacement]">
    <option value="Plasma Cannon Turret [emplacement]">
    <option value="Plasma Rifle Turret [emplacement]">
    <option value="PPC Turret [emplacement]">
    <option value="PPC w Capacitor Turret [emplacement]">
    <option value="ProtoMech AC/2 Turret [emplacement]">
    <option value="ProtoMech AC/4 Turret [emplacement]">
    <option value="ProtoMech AC/8 Turret [emplacement]">
    <option value="RAC/2 Turret [emplacement]">
    <option value="RAC/5 Turret [emplacement]">
    <option value="RISC APDS Turret [emplacement]">
    <option value="Rocket Launcher/1 Turret [emplacement]">
    <option value="Rocket Launcher/10 Turret [emplacement]">
    <option value="Rocket Launcher/15 Turret [emplacement]">
    <option value="Rocket Launcher/2 Turret [emplacement]">
    <option value="Rocket Launcher/20 Turret [emplacement]">
    <option value="Rocket Launcher/3 Turret [emplacement]">
    <option value="Rocket Launcher/4 Turret [emplacement]">
    <option value="Rocket Launcher/5 Turret [emplacement]">
    <option value="Silver Bullet Gauss Turret [emplacement]">
    <option value="Small Chemical Laser Turret [emplacement]">
    <option value="Small ER Pulse Laser Turret [emplacement]">
    <option value="Small Laser Turret [emplacement]">
    <option value="Small Pulse Laser Turret [emplacement]">
    <option value="Small Re-Engineered Laser Turret [emplacement]">
    <option value="Small VSP Laser Turret [emplacement]">
    <option value="Small X-Pulse Laser Turret [emplacement]">
    <option value="Sniper Artillery Cannon Turret [emplacement]">
    <option value="Sniper Artillery Turret [emplacement]">
    <option value="Snub-Nosed PPC [emplacement]">
    <option value="Snub-Nosed PPC w Capacitor Turret [emplacement]">
    <option value="SRM/2 Turret [emplacement]">
    <option value="SRM/2 Turret (w/ Artemis IV) [emplacement]">
    <option value="SRM/2 Turret (w/ Artemis V) [emplacement]">
    <option value="SRM/4 Turret [emplacement]">
    <option value="SRM/4 Turret (w/ Artemis IV) [emplacement]">
    <option value="SRM/4 Turret (w/ Artemis V) [emplacement]">
    <option value="SRM/6 Turret [emplacement]">
    <option value="SRM/6 Turret (w/ Artemis IV) [emplacement]">
    <option value="SRM/6 Turret (w/ Artemis V) [emplacement]">
    <option value="Streak LRM/10 Turret [emplacement]">
    <option value="Streak LRM/15 Turret [emplacement]">
    <option value="Streak LRM/20 Turret [emplacement]">
    <option value="Streak LRM/5 Turret [emplacement]">
    <option value="Streak SRM/2 Turret [emplacement]">
    <option value="Streak SRM/4 Turret [emplacement]">
    <option value="Streak SRM/6 Turret [emplacement]">
    <option value="TAG Turret [emplacement]">
    <option value="Thumper Artillery Cannon Turret [emplacement]">
    <option value="Thumper Artillery Turret [emplacement]">
    <option value="Thunderbolt 10 Turret [emplacement]">
    <option value="Thunderbolt 15 Turret [emplacement]">
    <option value="Thunderbolt 20 Turret [emplacement]">
    <option value="Thunderbolt 5 Turret [emplacement]">
    <option value="TSEMP Cannon Turret [emplacement]">
    <option value="UAC/10 Turret [emplacement]">
    <option value="UAC/2 Turret [emplacement]">
    <option value="UAC/20 Turret [emplacement]">
    <option value="UAC/5 Turret [emplacement]">
    <option value="Watchdog ECM Bunker [emplacement]">
</datalist>
        <div class="card">
            <h3 style="margin-top:0;">Post New Contract</h3>
            <form hx-post="/contract" hx-target="#board" hx-swap="afterbegin">
                <div class="form-grid">
                    <div>
                        <label>Date & Time:</label>
                        <input type="datetime-local" name="match_time" required>
                        <input type="hidden" name="match_epoch" id="match_epoch_input" value="">
                    </div>
                    <div>
                        <label>Target BV:</label>
                        <input type="number" name="bv2" placeholder="e.g. 5000">
                    </div>
                    <div>
                        <label>Mission Type:</label>
                        <select name="mission_type">
                            <option value="Skirmish">Stand-Up Fight</option>
                            <option value="Breakthrough">Objective Raid</option>
                            <option value="Base Assault">Extraction</option>
                            <option value="Reconnaisance">Hold the Line</option>
                            <option value="Campaign Scenario">Campaign Scenario</option>
                        </select>
                    </div>
                    <div>
                        <label>Force Composition:</label>
                        <select name="force_comp">
                            <option value="BattleMech Only">BattleMech Only</option>
                            <option value="Combined Arms">Combined Arms</option>
                        </select>
                    </div>
                    
                    <div style="grid-column: span 2; border: 1px dashed #39d353; padding: 10px;">
                        <label>Host Force Roster:</label>
                        <div style="display:flex; gap:10px; margin-top:5px;">
                            <input type="text" id="host-unit-search" list="unit-datalist" placeholder="Search Unit (e.g. Atlas AS7-D)..." style="margin-bottom:0;">
                            <button type="button" onclick="addUnit('host')" style="margin-bottom:0; width:auto;">ADD TO ROSTER</button>
                        </div>
                        <ul id="host-roster-list" style="list-style: none; padding: 0; margin: 10px 0; font-size: 0.9em;"></ul>
                        <input type="hidden" name="host_force" id="host_force_input" value="" required>
                    </div>

                    <div style="grid-column: span 2;">
                        <label>Host Assets (Optional):</label>
                        <input type="text" name="host_assets" placeholder="e.g. Desert Mats, Tokens, 3D Terrain">
                    </div>
                    
                    <div>
                        <label>Era:</label>
                        <select name="era">
                            <option value="ilClan">ilClan</option>
                            <option value="Dark Age">Dark Age</option>
                            <option value="Jihad">Jihad</option>
                            <option value="Civil War">Civil War</option>
                            <option value="Clan Invasion">Clan Invasion</option>
                            <option value="Succession Wars">Succession Wars</option>
                            <option value="Star League">Star League</option>
                        </select>
                    </div>
                    <div>
                        <label>Intel Level:</label>
                        <select name="intel_level">
                            <option value="Full Sweep (Revealed)">Full Sweep (Revealed)</option>
                            <option value="Partial Intercept">Partial Intercept</option>
                            <option value="Total Blackout (Blind)">Total Blackout (Blind)</option>
                        </select>
                    </div>
                    <div style="grid-column: span 2;">
                        <label>Ruleset:</label>
                        <select name="ruleset" hx-get="/ui/ruleset-options" hx-target="#optional-rules" hx-swap="innerHTML">
                            <option value="Core Rules (2026)">Core Rules (2026)</option>
                            <option value="Total Warfare">Total Warfare</option>
                            <option value="Introductory">Introductory</option>
                        </select>
                    </div>
                </div>

                <div id="optional-rules" class="checkbox-group">
                    <label><input type="checkbox" name="rule_battlefield_support_assets"> Battlefield Support Assets</label>
                    <label><input type="checkbox" name="rule_battlefield_support_strikes"> Battlefield Support Strikes</label>
                </div>

                <button type="submit">TRANSMIT CONTRACT</button>
            </form>
        </div>

        <h3>:: OPEN BOUNTIES ::</h3>
        <div id="board">
    "##,
        current_user = current_user
    );

    let footer = r##"
        </div>

        <script>
            function addUnit(prefix, id = '') {
                const searchId = id ? `${prefix}-unit-search-${id}` : `${prefix}-unit-search`;
                const listId = id ? `${prefix}-roster-list-${id}` : `${prefix}-roster-list`;
                const hiddenId = id ? `${prefix}_force_input_${id}` : `${prefix}_force_input`;
                
                const searchInput = document.getElementById(searchId);
                const val = searchInput.value;
                if (!val) return;

                const list = document.getElementById(listId);
                const hiddenInput = document.getElementById(hiddenId);

                const li = document.createElement('li');
                li.style.borderLeft = "2px solid #39d353";
                li.style.paddingLeft = "5px";
                li.style.marginBottom = "5px";
                li.style.display = "flex";
                li.style.justifyContent = "space-between";
                li.style.alignItems = "center";
                
                const textSpan = document.createElement('span');
                textSpan.innerText = val;
                li.appendChild(textSpan);
                
                const removeBtn = document.createElement('button');
                removeBtn.innerText = 'X';
                removeBtn.style.padding = '2px 5px';
                removeBtn.style.marginLeft = '10px';
                removeBtn.style.background = 'transparent';
                removeBtn.style.color = '#ff7b72';
                removeBtn.style.border = '1px solid #ff7b72';
                removeBtn.type = 'button';
                
                removeBtn.onclick = () => { 
                    li.remove(); 
                    updateHidden(list, hiddenInput);
                };
                
                li.appendChild(removeBtn);
                list.appendChild(li);

                updateHidden(list, hiddenInput);
                searchInput.value = '';
            }

            function updateHidden(listEl, hiddenEl) {
                const units = [];
                listEl.querySelectorAll('li span').forEach(span => units.push(span.innerText));
                hiddenEl.value = units.join(',');
            }

            function updateMatchEpoch() {
                const timeInput = document.querySelector('input[name="match_time"]');
                const epochInput = document.getElementById('match_epoch_input');
                if (!timeInput || !epochInput || !timeInput.value) return;

                const timestamp = new Date(timeInput.value).getTime();
                if (!Number.isNaN(timestamp)) {
                    epochInput.value = String(Math.floor(timestamp / 1000));
                }
            }

            document.addEventListener("DOMContentLoaded", () => {
                const timeInput = document.querySelector('input[name="match_time"]');
                if (timeInput && !timeInput.value) {
                    const today = new Date();
                    const yyyy = today.getFullYear();
                    const mm = String(today.getMonth() + 1).padStart(2, '0');
                    const dd = String(today.getDate()).padStart(2, '0');
                    timeInput.value = `${yyyy}-${mm}-${dd}T17:00`;
                }

                updateMatchEpoch();

                if (timeInput) {
                    timeInput.addEventListener('change', updateMatchEpoch);
                }

                const contractForm = document.querySelector('form[hx-post="/contract"]');
                if (contractForm) {
                    contractForm.addEventListener('submit', updateMatchEpoch);
                }
            });

            function updateCountdowns() {
                document.querySelectorAll('.countdown').forEach(el => {
                    const target = new Date(el.dataset.time).getTime();
                    const now = new Date().getTime();
                    const diff = target - now;
                    
                    if (isNaN(target)) return;
                    
                    if (diff < 0) { 
                        el.innerText = "[ DEPLOYMENT IN PROGRESS ]"; 
                        return; 
                    }
                    
                    const d = Math.floor(diff / (1000 * 60 * 60 * 24));
                    const h = Math.floor((diff % (1000 * 60 * 60 * 24)) / (1000 * 60 * 60));
                    const m = Math.floor((diff % (1000 * 60 * 60)) / (1000 * 60));
                    el.innerText = `T-MINUS: ${d}d ${h}h ${m}m`;
                });
            }
            setInterval(updateCountdowns, 60000); 
            updateCountdowns(); 

            document.body.addEventListener('htmx:afterSwap', function(evt) {
                updateCountdowns();
            });
        </script>
    </body>
    </html>
    "##;

    format!("{}{}{}", header, rows, footer)
}

// --- HANDLERS ---

async fn get_board(jar: CookieJar, State(state): State<AppState>) -> Html<String> {
    cleanup_expired_contracts(&state.db);

    let current_user = match jar.get("username") {
        Some(cookie) => cookie.value().to_string(),
        None => return Html(r##"
            <!DOCTYPE html>
            <html lang="en">
            <head>
                <title>LGS ComStar Terminal - Login</title>
                <style>
                    body { background-color: #0d1117; color: #39d353; font-family: monospace; max-width: 400px; margin: 100px auto; padding: 20px; text-align: center; }
                    .card { border: 1px solid #39d353; padding: 20px; background: #161b22; }
                    input, button { background: #0d1117; border: 1px solid #39d353; color: #39d353; padding: 10px; font-family: monospace; margin-bottom: 15px; width: 100%; box-sizing: border-box; }
                    button { cursor: pointer; font-weight: bold; }
                    button:hover { background: #39d353; color: #0d1117; }
                </style>
            </head>
            <body>
                <div class="card">
                    <h2>:: COMSTAR UPLINK ::</h2>
                    <p>Identify yourself to access the Mercenary Board.</p>
                    <form method="POST" action="/login">
                        <input type="text" name="username" placeholder="Enter Callsign" required>
                        <button type="submit">ESTABLISH UPLINK</button>
                    </form>
                </div>
            </body>
            </html>
        "##.to_string()),
    };

    let mut contracts = Vec::new();
    for item in state.db.iter() {
        if let Ok((_, value)) = item {
            let contract: Contract = serde_json::from_slice(&value).unwrap();
            contracts.push(contract);
        }
    }

    Html(render_board(&contracts, &current_user))
}

async fn login(jar: CookieJar, Form(input): Form<LoginForm>) -> (CookieJar, Html<String>) {
    let updated_jar = jar.add(Cookie::new("username", input.username));
    let response = Html(r##"<script>window.location.href = "/";</script>"##.to_string());
    (updated_jar, response)
}

async fn get_ruleset_options(Query(query): Query<RulesetQuery>) -> Html<String> {
    let html = if query.ruleset.contains("Total Warfare") {
        r##"
        <label><input type="checkbox" name="rule_floating_crits"> Floating Criticals</label>
        <label><input type="checkbox" name="rule_forced_withdrawal"> Forced Withdrawal</label>
        <label><input type="checkbox" name="rule_backward_level_change"> Backwards Level Change</label>
        <label><input type="checkbox" name="rule_initiative_die"> Initiative Die</label>
        <label><input type="checkbox" name="rule_careful_stand"> Careful Stand</label>
        <label><input type="checkbox" name="rule_sprinting"> Sprinting</label>
        <label><input type="checkbox" name="rule_expanded_arm_flipping"> Expanded Arm Flippping</label>
        <label><input type="checkbox" name="rule_front-loaded_deployment"> Front-Loaded Deployment</label>
        "##
    } else if query.ruleset.contains("Core Rules (2026)") {
        r##"
        <label><input type="checkbox" name="rule_battlefield_support_assets"> Battlefield Support Assets</label>
        <label><input type="checkbox" name="rule_battlefield_support_strikes"> Battlefield Support Strikes</label>
        <label><input type="checkbox" name="rule_initiative_die"> Initiative Die</label>
        "##
    } else {
        r##"
    "##
    };
    Html(html.to_string())
}

async fn create_contract(
    jar: CookieJar,
    State(state): State<AppState>,
    Form(input): Form<HashMap<String, String>>,
) -> Html<String> {
    let current_user = jar
        .get("username")
        .map(|c| c.value().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let mut checked_rules: Vec<String> = input
        .iter()
        .filter(|(k, _)| k.starts_with("rule_"))
        .map(|(k, _)| k.replace("rule_", "").replace("_", " ").to_uppercase())
        .collect();
    checked_rules.sort();

    let new_id = state.db.generate_id().unwrap();

    let match_epoch = input
        .get("match_epoch")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);

    let expires_at = if match_epoch > 0 {
        match_epoch.saturating_add(2 * 60 * 60)
    } else {
        0
    };

    let new_contract = Contract {
        id: new_id,
        host: current_user.clone(),
        host_force: input.get("host_force").cloned().unwrap_or_default(),
        host_assets: input.get("host_assets").cloned().unwrap_or_default(),
        challenger: None,
        challenger_force: None,
        challenger_assets: None,
        mission_type: input.get("mission_type").cloned().unwrap_or_default(),
        bv2: input.get("bv2").cloned().unwrap_or_default(),
        era: input.get("era").cloned().unwrap_or_default(),
        map: input.get("map").cloned().unwrap_or_default(),
        intel_level: input.get("intel_level").cloned().unwrap_or_default(),
        match_time: input.get("match_time").cloned().unwrap_or_default(),
        expires_at,
        force_comp: input.get("force_comp").cloned().unwrap_or_default(),
        ruleset: input.get("ruleset").cloned().unwrap_or_default(),
        optional_rules: checked_rules,
        status: "Open".to_string(),
        comments: Vec::new(),
    };

    state
        .db
        .insert(
            new_id.to_be_bytes(),
            serde_json::to_vec(&new_contract).unwrap(),
        )
        .unwrap();

    Html(render_contract(&new_contract, &current_user))
}

async fn accept_contract(
    jar: CookieJar,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(input): Form<AcceptForm>,
) -> Html<String> {
    let current_user = jar
        .get("username")
        .map(|c| c.value().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let raw_bytes = state.db.get(id.to_be_bytes()).unwrap().unwrap();
    let mut contract: Contract = serde_json::from_slice(&raw_bytes).unwrap();

    contract.status = "Accepted".to_string();
    contract.challenger = Some(current_user.clone());
    contract.challenger_force = Some(input.challenger_force);
    contract.challenger_assets = Some(input.challenger_assets);

    state
        .db
        .insert(id.to_be_bytes(), serde_json::to_vec(&contract).unwrap())
        .unwrap();

    Html(render_contract(&contract, &current_user))
}

async fn withdraw_contract(
    jar: CookieJar,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Html<String> {
    let current_user = jar
        .get("username")
        .map(|c| c.value().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let raw_bytes = state.db.get(id.to_be_bytes()).unwrap().unwrap();
    let mut contract: Contract = serde_json::from_slice(&raw_bytes).unwrap();

    // Only allow the active challenger to withdraw themselves
    if contract.challenger.as_deref() == Some(current_user.as_str()) {
        contract.status = "Open".to_string();
        contract.challenger = None;
        contract.challenger_force = None;
        contract.challenger_assets = None;
        state
            .db
            .insert(id.to_be_bytes(), serde_json::to_vec(&contract).unwrap())
            .unwrap();
    }

    Html(render_contract(&contract, &current_user))
}

async fn transfer_contract(
    jar: CookieJar,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Html<String> {
    let current_user = jar
        .get("username")
        .map(|c| c.value().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let raw_bytes = state.db.get(id.to_be_bytes()).unwrap().unwrap();
    let mut contract: Contract = serde_json::from_slice(&raw_bytes).unwrap();

    // Only the host can hand over command, and only if a challenger exists
    if contract.host == current_user && contract.challenger.is_some() {
        contract.host = contract.challenger.take().unwrap();
        contract.host_force = contract.challenger_force.take().unwrap_or_default();
        contract.host_assets = contract.challenger_assets.take().unwrap_or_default();
        contract.status = "Open".to_string();

        state
            .db
            .insert(id.to_be_bytes(), serde_json::to_vec(&contract).unwrap())
            .unwrap();
    }

    Html(render_contract(&contract, &current_user))
}

async fn delete_contract(
    jar: CookieJar,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Html<String> {
    let current_user = jar
        .get("username")
        .map(|c| c.value().to_string())
        .unwrap_or_default();

    if let Ok(Some(raw_bytes)) = state.db.get(id.to_be_bytes()) {
        let contract: Contract = serde_json::from_slice(&raw_bytes).unwrap();
        if contract.host == current_user {
            state.db.remove(id.to_be_bytes()).unwrap();
        }
    }

    Html(String::new())
}

async fn add_comment(
    jar: CookieJar,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Form(input): Form<CommentForm>,
) -> Html<String> {
    let current_user = jar
        .get("username")
        .map(|c| c.value().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let raw_bytes = state.db.get(id.to_be_bytes()).unwrap().unwrap();
    let mut contract: Contract = serde_json::from_slice(&raw_bytes).unwrap();

    contract.comments.push(Comment {
        author: current_user.clone(),
        text: input.text,
    });

    state
        .db
        .insert(id.to_be_bytes(), serde_json::to_vec(&contract).unwrap())
        .unwrap();

    Html(render_contract(&contract, &current_user))
}

// --- ROUTER ---

#[tokio::main]
async fn main() {
    let db = sled::open("mercenary_board.db").expect("Failed to open Sled database");

    let state = AppState { db };

    let cleanup_db = state.db.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;
            cleanup_expired_contracts(&cleanup_db);
        }
    });

    let app = Router::new()
        .route("/", get(get_board))
        .route("/login", post(login))
        .route("/ui/ruleset-options", get(get_ruleset_options))
        .route("/contract", post(create_contract))
        .route("/contract/:id", delete(delete_contract))
        .route("/contract/:id/accept", post(accept_contract))
        .route("/contract/:id/withdraw", post(withdraw_contract))
        .route("/contract/:id/transfer", post(transfer_contract))
        .route("/contract/:id/comment", post(add_comment))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("ComStar terminal online at http://localhost:3000");
    axum::serve(listener, app).await.unwrap();
}
