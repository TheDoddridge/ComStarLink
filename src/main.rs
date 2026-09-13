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

mod units;

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
            {unit_datalist}
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
        current_user = current_user,
        unit_datalist = units::UNIT_DATALIST_HTML
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
