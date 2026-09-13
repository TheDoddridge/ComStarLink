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
    #[serde(default)]
    pv: String,
    #[serde(default)]
    bsp: String,

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

// --- HELPER FUNCTIONS ---

fn format_force(force_string: &str, intel_level: &str, is_owner: bool) -> String {
    if force_string.is_empty() {
        return "<span class='term-dim'>[ NONE SPECIFIED ]</span>".to_string();
    }

    if !is_owner {
        if intel_level.contains("Blackout") {
            return "<span class='term-alert'>[ CLASSIFIED / SIGNAL LOST ]</span>".to_string();
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
                return "<span class='term-warn'>[ ENCRYPTED / UNKNOWN ]</span>".to_string();
            }
            return format!(
                "<span class='term-warn'>[ PARTIAL INTERCEPT: {} ]</span>",
                summary.join(", ")
            );
        }
    }

    // Full Sweep OR Current User is the Owner
    let mut formatted = force_string.replace(",", "<br>• ");

    if is_owner && (intel_level.contains("Blackout") || intel_level.contains("Intercept")) {
        formatted = format!(
            "<span class='term-data'>{}</span> <br><span class='term-dim' style='margin-top: 10px; display: inline-block;'><em>(Visible only to you - Intel level hides this from others)</em></span>", 
            formatted
        );
    } else {
        formatted = format!("<span class='term-data'>{}</span>", formatted);
    }

    formatted
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

        if contract.expires_at > 0 && now >= contract.expires_at {
            let _ = db.remove(key);
        }
    }
}

// --- HTML RENDERING ---

fn render_contract(c: &Contract, current_user: &str) -> String {
    let is_host = current_user == c.host;
    let is_challenger = Some(current_user.to_string()) == c.challenger;

    let host_force_display = format_force(&c.host_force, &c.intel_level, is_host);
    let host_assets_display = if c.host_assets.is_empty() {
        "NONE".to_string()
    } else {
        c.host_assets.clone()
    };
    let display_time_fallback = c.match_time.replace("T", " ");

    // Dynamic Display for BV / PV / BSP
    let mut points_display = String::new();
    if c.ruleset == "Alpha Strike" {
        let pv_val = if c.pv.is_empty() {
            "OPEN".to_string()
        } else {
            c.pv.clone()
        };
        points_display.push_str(&format!(r#"<div><span class="term-label">TARGET PV:</span> <span class="term-data">{}</span></div>"#, pv_val));
    } else {
        let bv_val = if c.bv2.is_empty() {
            "OPEN".to_string()
        } else {
            c.bv2.clone()
        };
        points_display.push_str(&format!(r#"<div><span class="term-label">TARGET BV:</span> <span class="term-data">{}</span></div>"#, bv_val));
    }

    if !c.bsp.is_empty() {
        points_display.push_str(&format!(r#"<div><span class="term-label">TARGET BSP:</span> <span class="term-data">{}</span></div>"#, c.bsp));
    }

    // 1. Action Area (Accept / Transfer / Withdraw logic)
    let action_html = if c.status == "Open" {
        if is_host {
            r##"<div class="panel-divider">
                <em class="term-dim">[ AWAITING CHALLENGER SIGNATURES... ]</em>
            </div>"##
                .to_string()
        } else {
            format!(
                r##"<form hx-post="/contract/{id}/accept" hx-target="#contract-{id}" hx-swap="outerHTML" class="panel-divider">
                    <strong class="term-label">:: ACCEPT CONTRACT ::</strong>
                    
                    <div class="panel panel-challenger" style="margin-top: 15px;">
                        <label class="term-label">CHALLENGER FORCE ROSTER:</label>
                        <div style="display:flex; gap:10px; margin-top:5px;">
                            <input type="text" id="challenger-unit-search-{id}" list="unit-datalist" placeholder="Search Unit..." style="margin-bottom:0;">
                            <button type="button" onclick="addUnit('challenger', '{id}')" style="margin-bottom:0; width:auto;" class="btn-challenger">ADD</button>
                        </div>
                        <ul id="challenger-roster-list-{id}" class="roster-list"></ul>
                        <input type="hidden" name="challenger_force" id="challenger_force_input_{id}" value="">
                        
                        <label class="term-label" style="margin-top: 15px; display: block;">ASSETS (OPTIONAL):</label>
                        <input type="text" name="challenger_assets" placeholder="e.g. Tokens, Maps" style="margin-top: 5px;">
                    </div>
                    <button type="submit" class="btn-challenger" style="width: 100%;">LOCK MATCH</button>
                </form>"##,
                id = c.id
            )
        }
    } else {
        let mut action_buttons = String::new();

        if is_host {
            action_buttons.push_str(&format!(
                r##"<button hx-post="/contract/{id}/transfer" hx-target="#contract-{id}" hx-swap="outerHTML" class="btn-primary" style="width: auto; margin-bottom: 0; padding: 5px 10px;">
                    TRANSFER COMMAND TO CHALLENGER
                </button>"##, id = c.id
            ));
        } else if is_challenger {
            action_buttons.push_str(&format!(
                r##"<button hx-post="/contract/{id}/withdraw" hx-target="#contract-{id}" hx-swap="outerHTML" class="btn-alert" style="width: auto; margin-bottom: 0; padding: 5px 10px;">
                    WITHDRAW
                </button>"##, id = c.id
            ));
        }

        let challenger_force_display = format_force(
            c.challenger_force.as_deref().unwrap_or(""),
            &c.intel_level,
            is_challenger,
        );
        let challenger_assets = c.challenger_assets.as_deref().unwrap_or("").trim();
        let challenger_assets_display = if challenger_assets.is_empty() {
            "NONE".to_string()
        } else {
            format!("<span class='term-data'>{}</span>", challenger_assets)
        };

        let challenger_info = format!(
            r##"<div class="panel panel-challenger">
                <div style="margin-bottom: 10px;"><strong class="term-label">:: CHALLENGER IDENTIFIED ::</strong> <span class="term-name">{challenger}</span></div>
                <div style="margin-bottom: 15px;">• {force}</div>
                <div><span class="term-label">ASSETS:</span> {assets}</div>
            </div>"##,
            challenger = c.challenger.as_deref().unwrap_or("UNKNOWN"),
            force = challenger_force_display,
            assets = challenger_assets_display,
        );

        format!(
            r##"<div class="panel-divider">
                <div style="display: flex; gap: 15px; align-items: center; margin-bottom: 15px;">
                    <strong class="term-alert">[ MATCH LOCKED - AWAITING DEPLOYMENT ]</strong>
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
        let is_host_comment = comment.author == c.host;
        let prefix = if is_host_comment { "[HOST]" } else { "[CHAL]" };

        comments_html.push_str(&format!(
            r##"<div style="margin-top: 10px;">
                <strong class="term-name">{prefix} {author}:</strong><br> <span class="term-data">{text}</span>
            </div>"##,
            prefix = prefix,
            author = comment.author,
            text = comment.text
        ));
    }

    if comments_html.is_empty() {
        comments_html = "<em class='term-dim'>[ NO ENCRYPTED COMMS ]</em>".to_string();
    }

    let comments_section = format!(
        r##"<div class="panel" style="margin-top: 20px;">
            <strong class="term-label">:: COMMS CHANNEL ::</strong>
            <div style="margin: 15px 0; max-height: 200px; overflow-y: auto;">{comments_html}</div>
            <form hx-post="/contract/{id}/comment" hx-target="#contract-{id}" hx-swap="outerHTML" style="display: flex; gap: 10px; margin-bottom: 0;">
                <input type="text" name="text" placeholder="Transmit message..." required style="margin-bottom: 0; text-transform: none;">
                <button type="submit" class="btn-primary" style="margin-bottom: 0; width: auto; padding: 0 20px;">SEND</button>
            </form>
        </div>"##,
        id = c.id,
        comments_html = comments_html
    );

    // 3. Optional Rules and Final Assembly
    let optional_rules_html = if c.optional_rules.is_empty() {
        "NONE".to_string()
    } else {
        format!(
            "<span class='term-data'>{}</span>",
            c.optional_rules.join(", ")
        )
    };
    let cancel_button = if is_host {
        format!(
            r##"<button hx-delete="/contract/{id}" hx-confirm="SCRUB MISSION? THIS CANNOT BE UNDONE." hx-target="#contract-{id}" hx-swap="outerHTML" class="btn-alert" style="margin-top: 20px; width: 100%;">
                SCRUB MISSION (CANCEL CONTRACT)
            </button>"##,
            id = c.id
        )
    } else {
        String::new()
    };

    format!(
        r##"<div class="card" id="contract-{id}">
            <div class="card-header">
                <span><strong>CONTRACT #{id}</strong> // HOST: <span class="term-name">{host}</span></span>
                <span class="term-timer">
                    [ <span class="exact-time" data-time="{match_time}">{display_time_fallback}</span> | 
                    <span class="countdown" data-time="{match_time}">CALCULATING...</span> ]
                </span>
            </div>
            
            <div class="form-grid">
                <div class="panel panel-info">
                    <div><span class="term-label">MISSION:</span> <span class="term-data">{mission_type}</span></div>
                    {points_display}
                    <div><span class="term-label">FORCE COMP:</span> <span class="term-data">{force_comp}</span></div>
                    <div><span class="term-label">ERA:</span> <span class="term-data">{era}</span></div>
                    <div><span class="term-label">INTEL:</span> <span class="term-data">{intel_level}</span></div>
                    <div style="margin-top: 10px;"><span class="term-label">RULESET:</span> <span class="term-data">{ruleset}</span></div>
                    <div><span class="term-label">OPT RULES:</span> {optional_rules_html}</div>
                </div>

                <div class="panel panel-host">
                    <div style="margin-bottom: 10px;"><strong class="term-label">:: HOST FORCE DEPLOYMENT ::</strong></div>
                    <div style="margin-bottom: 15px;">• {host_force_display}</div>
                    <div><span class="term-label">ASSETS:</span> <span class="term-data">{host_assets}</span></div>
                </div>
            </div>
            
            {action_html}
            {comments_section}
            {cancel_button}
        </div>"##,
        id = c.id,
        host = c.host,
        match_time = c.match_time,
        display_time_fallback = display_time_fallback,
        mission_type = c.mission_type,
        points_display = points_display,
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
            :root {{
                --term-bg: #0d1117;        /* Deep Space Black/Blue */
                --term-panel: #161b22;     /* Slightly lighter panel background */
                --term-border: #30363d;    /* Subdued structural borders */
                --term-label: #58a6ff;     /* Bright Cyan for field names */
                --term-data: #c9d1d9;      /* Crisp Off-White for field contents */
                --term-name: #e3b341;      /* Gold for all User Names */
                --term-accent: #3fb950;    /* Phosphor Green for accents/host */
                --term-alert: #ff7b72;     /* Red for warnings/deletions */
                --term-warn: #d29922;      /* Orange/Yellow for general alerts */
                --term-dim: #8b949e;       /* Grey for secondary text */
                --term-purple: #bc8cff;    /* Purple for Challenger specifics */
                --term-timer: #ff79c6;     /* Retro Magenta for exact times/countdowns */
            }}

            body {{ 
                background-color: var(--term-bg); 
                color: var(--term-data); 
                font-family: 'Courier New', Courier, monospace; 
                font-size: 16px;
                max-width: 900px; 
                margin: 0 auto; 
                padding: 20px; 
                text-shadow: 0 0 1px rgba(201, 209, 217, 0.2);
                line-height: 1.4;
            }}

            h2, h3 {{ margin-top: 0; color: var(--term-label); text-transform: uppercase; font-weight: bold; letter-spacing: 1px; }}
            
            /* Semantic Color Classes */
            .term-label {{ color: var(--term-label); text-transform: uppercase; font-weight: bold; margin-right: 5px; }}
            .term-data {{ color: var(--term-data); }}
            .term-name {{ color: var(--term-name); font-weight: bold; text-shadow: 0 0 3px rgba(227, 179, 65, 0.3); }}
            .term-alert {{ color: var(--term-alert); font-weight: bold; }}
            .term-warn {{ color: var(--term-warn); font-weight: bold; }}
            .term-dim {{ color: var(--term-dim); }}
            .term-timer {{ color: var(--term-timer); font-weight: bold; text-shadow: 0 0 3px rgba(255, 121, 198, 0.4); }}

            /* Structural Elements */
            .card {{ 
                border: 1px solid var(--term-border); 
                padding: 20px; 
                margin-bottom: 30px; 
                background: var(--term-bg); 
            }}

            .card-header {{
                display: flex; 
                justify-content: space-between; 
                border-bottom: 1px dashed var(--term-border); 
                margin-bottom: 20px; 
                padding-bottom: 10px;
            }}

            .panel {{ 
                padding: 15px; 
                background: var(--term-panel); 
                border: 1px dashed var(--term-border); 
            }}
            .panel-info {{ border-left: 3px solid var(--term-label); }}
            .panel-host {{ border-left: 3px solid var(--term-accent); }}
            .panel-challenger {{ border-left: 3px solid var(--term-purple); }}
            
            .panel-divider {{
                margin-top: 20px; 
                padding-top: 20px; 
                border-top: 1px dashed var(--term-border);
            }}

            /* Forms & Inputs */
            input, select, button {{ 
                background: var(--term-bg); 
                border: 1px solid var(--term-border); 
                color: var(--term-data); 
                padding: 10px; 
                font-family: inherit; 
                font-size: 1rem;
                margin-bottom: 15px; 
                width: 100%; 
                box-sizing: border-box; 
            }}
            
            input::placeholder {{ color: var(--term-dim); }}
            input:focus, select:focus {{ outline: none; border-color: var(--term-label); box-shadow: 0 0 5px rgba(88, 166, 255, 0.2); }}

            /* Invert the black calendar icon to an off-white matching var(--term-data) */
            input[type="datetime-local"]::-webkit-calendar-picker-indicator {{
                cursor: pointer;
                filter: invert(85%) sepia(10%) saturate(100%) hue-rotate(180deg) brightness(95%) contrast(90%);
            }}

            /* Standardized Hollow Buttons that fill on hover */
            button {{ cursor: pointer; font-weight: bold; transition: background-color 0.1s, color 0.1s; text-transform: uppercase; background: transparent; }}
            
            .btn-primary {{ color: var(--term-label); border-color: var(--term-label); }}
            .btn-primary:hover {{ background: var(--term-label); color: var(--term-bg); }}
            
            .btn-outline {{ color: var(--term-accent); border-color: var(--term-accent); }}
            .btn-outline:hover {{ background: var(--term-accent); color: var(--term-bg); }}
            
            .btn-challenger {{ color: var(--term-purple); border-color: var(--term-purple); }}
            .btn-challenger:hover {{ background: var(--term-purple); color: var(--term-bg); }}

            .btn-alert {{ color: var(--term-alert); border-color: var(--term-alert); }}
            .btn-alert:hover {{ background: var(--term-alert); color: var(--term-bg); }}

            /* Grid Layouts */
            .form-grid {{ display: grid; grid-template-columns: 1fr 1fr; gap: 20px; align-items: start; }}
            
            .checkbox-group {{ margin: 15px 0; border: 1px dashed var(--term-border); padding: 15px; background: var(--term-panel); }}
            .checkbox-group label {{ display: flex; align-items: center; cursor: pointer; margin-bottom: 10px; color: var(--term-data); }}
            .checkbox-group input {{ width: auto; margin-right: 15px; margin-bottom: 0; }}
            .checkbox-group label:last-child {{ margin-bottom: 0; }}

            .top-bar {{ 
                display: flex; 
                justify-content: space-between; 
                align-items: center; 
                border-bottom: 2px solid var(--term-label); 
                margin-bottom: 30px; 
                padding-bottom: 10px; 
            }}

            /* Rosters */
            .roster-list {{ list-style: none; padding: 0; margin: 10px 0; }}
            .roster-list li {{ border-left: 2px solid var(--term-accent); padding-left: 10px; margin-bottom: 8px; display: flex; justify-content: space-between; align-items: center; background: var(--term-panel); padding: 8px; color: var(--term-data); }}
            .roster-list li button {{ width: auto; margin: 0; padding: 4px 10px; }}
        </style>
    </head>
    <body>
        <div class="top-bar">
            <h2>:: MERCENARY REVIEW BOARD ::</h2>
            <span>LOGGED IN AS: <strong class="term-name">{current_user}</strong></span>
        </div>
        
        <datalist id="unit-datalist">
            {unit_datalist}
        </datalist>

        <div class="card" style="border-top: 3px solid var(--term-label);">
            <h3>:: INITIALIZE NEW CONTRACT ::</h3>
            <form hx-post="/contract" hx-target="#board" hx-swap="afterbegin">
                <div class="form-grid">
                    <div>
                        <label class="term-label">DATE & TIME:</label>
                        <input type="datetime-local" name="match_time" required style="color: var(--term-data);">
                        <input type="hidden" name="match_epoch" id="match_epoch_input" value="">
                    </div>
                    
                    <div>
                        <label class="term-label">RULESET:</label>
                        <select name="ruleset" hx-get="/ui/ruleset-options" hx-target="#optional-rules" hx-swap="innerHTML" onchange="handleRulesetChange(this)" style="color: var(--term-data);">
                            <option value="Core Rules (2026)">Core Rules (2026)</option>
                            <option value="Total Warfare">Total Warfare</option>
                            <option value="Introductory">Introductory</option>
                            <option value="Alpha Strike">Alpha Strike</option>
                        </select>
                    </div>

                    <div id="bv-container">
                        <label class="term-label">TARGET BV:</label>
                        <input type="number" name="bv2" placeholder="e.g. 5000">
                    </div>

                    <div id="pv-container" style="display: none;">
                        <label class="term-label">TARGET PV:</label>
                        <input type="number" name="pv" placeholder="e.g. 250">
                    </div>

                    <div id="bsp-container" style="display: none;">
                        <label class="term-label">TARGET BSP:</label>
                        <input type="number" id="bsp-input" name="bsp" placeholder="e.g. 50">
                    </div>
                    
                    <div>
                        <label class="term-label">MISSION TYPE:</label>
                        <select name="mission_type" style="color: var(--term-data);">
                            <option value="Skirmish">Skirmish</option>
                            <option value="Breakthrough">Breakthrough</option>
                            <option value="Base Assault">Base Assault</option>
                            <option value="Reconnaisance">Reconnaisance</option>
                            <option value="Campaign Scenario">Campaign Scenario</option>
                        </select>
                    </div>
                    <div>
                        <label class="term-label">FORCE COMPOSITION:</label>
                        <select name="force_comp" style="color: var(--term-data);">
                            <option value="BattleMech Only">BattleMech Only</option>
                            <option value="Combined Arms">Combined Arms</option>
                        </select>
                    </div>
                    
                    <div class="panel panel-host" style="grid-column: span 2;">
                        <label class="term-label">HOST FORCE ROSTER:</label>
                        <div style="display:flex; gap:10px; margin-top:10px;">
                            <input type="text" id="host-unit-search" list="unit-datalist" placeholder="Search Unit (e.g. Atlas AS7-D)..." style="margin-bottom:0;">
                            <button type="button" onclick="addUnit('host')" style="margin-bottom:0; width:auto;" class="btn-outline">ADD TO ROSTER</button>
                        </div>
                        <ul id="host-roster-list" class="roster-list"></ul>
                        <input type="hidden" name="host_force" id="host_force_input" value="" required>
                    </div>

                    <div style="grid-column: span 2;">
                        <label class="term-label">HOST ASSETS (OPTIONAL):</label>
                        <input type="text" name="host_assets" placeholder="e.g. Desert Mats, Tokens, 3D Terrain">
                    </div>
                    
                    <div>
                        <label class="term-label">ERA:</label>
                        <select name="era" style="color: var(--term-data);">
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
                        <label class="term-label">INTEL LEVEL:</label>
                        <select name="intel_level" style="color: var(--term-data);">
                            <option value="Full Sweep (Revealed)">Full Sweep (Revealed)</option>
                            <option value="Partial Intercept">Partial Intercept</option>
                            <option value="Total Blackout (Blind)">Total Blackout (Blind)</option>
                        </select>
                    </div>
                </div>

                <div id="optional-rules" class="checkbox-group">
                    <label><input type="checkbox" name="rule_Battlefield_Support_Assets" onchange="toggleBsp()"> Battlefield Support Assets</label>
                    <label><input type="checkbox" name="rule_Battlefield_Support_Strikes" onchange="toggleBsp()"> Battlefield Support Strikes</label>
                </div>

                <button type="submit" class="btn-primary" style="width: 100%;">TRANSMIT CONTRACT</button>
            </form>
        </div>

        <h3 class="term-label">:: OPEN BOUNTIES ::</h3>
        <div id="board">
    "##,
        current_user = current_user,
        unit_datalist = units::UNIT_DATALIST_HTML
    );

    let footer = r##"
        </div>

        <script>
            function handleRulesetChange(select) {
                const bvContainer = document.getElementById('bv-container');
                const pvContainer = document.getElementById('pv-container');
                
                if (select.value === 'Alpha Strike') {
                    bvContainer.style.display = 'none';
                    pvContainer.style.display = 'block';
                } else {
                    bvContainer.style.display = 'block';
                    pvContainer.style.display = 'none';
                }
            }

            function toggleBsp() {
                const bsa = document.querySelector('input[name="rule_Battlefield_Support_Assets"]');
                const bss = document.querySelector('input[name="rule_Battlefield_Support_Strikes"]');
                const bspContainer = document.getElementById('bsp-container');
                
                if ((bsa && bsa.checked) || (bss && bss.checked)) {
                    bspContainer.style.display = 'block';
                } else {
                    bspContainer.style.display = 'none';
                    document.getElementById('bsp-input').value = ''; 
                }
            }
            
            window.toggleBsp = toggleBsp;

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
                
                // Color coordination for lists
                if (prefix === 'challenger') {
                    li.style.borderLeftColor = 'var(--term-purple)';
                }
                
                const textSpan = document.createElement('span');
                textSpan.innerText = val;
                li.appendChild(textSpan);
                
                const removeBtn = document.createElement('button');
                removeBtn.innerText = 'X';
                removeBtn.className = 'btn-alert';
                
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
            
            function formatExactTimes() {
                document.querySelectorAll('.exact-time:not(.initialized)').forEach(el => {
                    const d = new Date(el.dataset.time);
                    if (!isNaN(d)) {
                        el.innerText = d.toLocaleString(undefined, { 
                            weekday: 'short', month: 'short', day: 'numeric', 
                            hour: '2-digit', minute: '2-digit' 
                        }).toUpperCase();
                    }
                    el.classList.add('initialized');
                });
            }

            function updateCountdowns() {
                document.querySelectorAll('.countdown').forEach(el => {
                    const target = new Date(el.dataset.time).getTime();
                    const now = new Date().getTime();
                    const diff = target - now;
                    
                    if (isNaN(target)) return;
                    
                    if (diff < 0) { 
                        el.innerText = "T-MINUS: 0d 0h 0m"; 
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
            formatExactTimes();

            document.body.addEventListener('htmx:afterSwap', function(evt) {
                toggleBsp();
                updateCountdowns();
                formatExactTimes();
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
                    :root {
                        --term-bg: #0d1117;
                        --term-label: #58a6ff;
                        --term-data: #c9d1d9;
                        --term-border: #30363d;
                        --term-dim: #8b949e;
                    }
                    body { background-color: var(--term-bg); color: var(--term-data); font-family: 'Courier New', Courier, monospace; font-size: 16px; max-width: 400px; margin: 100px auto; padding: 20px; text-align: center; text-shadow: 0 0 1px rgba(201, 209, 217, 0.2); }
                    .card { border: 1px solid var(--term-border); padding: 30px; background: #161b22; border-top: 3px solid var(--term-label); }
                    input, button { background: transparent; border: 1px solid var(--term-border); color: var(--term-data); padding: 12px; font-family: inherit; font-size: 1rem; margin-bottom: 20px; width: 100%; box-sizing: border-box; }
                    input:focus { outline: none; border-color: var(--term-label); box-shadow: 0 0 5px rgba(88, 166, 255, 0.2); }
                    
                    button { cursor: pointer; font-weight: bold; color: var(--term-label); border-color: var(--term-label); transition: 0.1s; text-transform: uppercase; }
                    button:hover { background: var(--term-label); color: var(--term-bg); }
                    
                    .term-label { color: var(--term-label); text-transform: uppercase; margin-top: 0; letter-spacing: 1px;}
                    .term-dim { color: var(--term-dim); margin-bottom: 20px;}
                </style>
            </head>
            <body>
                <div class="card">
                    <h2 class="term-label">:: COMSTAR UPLINK ::</h2>
                    <p class="term-dim">IDENTIFY YOURSELF TO ACCESS THE MERCENARY BOARD.</p>
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
    let cookie = Cookie::build(("username", input.username))
        .path("/")
        .build();

    let updated_jar = jar.add(cookie);
    let response = Html(r##"<script>window.location.href = "/";</script>"##.to_string());

    (updated_jar, response)
}

async fn get_ruleset_options(Query(query): Query<RulesetQuery>) -> Html<String> {
    let html = if query.ruleset.contains("Total Warfare") {
        r##"
        <label><input type="checkbox" name="rule_Floating_Crits"> Floating Criticals</label>
        <label><input type="checkbox" name="rule_Forced_Withdrawal"> Forced Withdrawal</label>
        <label><input type="checkbox" name="rule_Backward_Level_Change"> Backwards Level Change</label>
        <label><input type="checkbox" name="rule_Initiative_Die"> Initiative Die</label>
        <label><input type="checkbox" name="rule_Careful_Stand"> Careful Stand</label>
        <label><input type="checkbox" name="rule_Sprinting"> Sprinting</label>
        <label><input type="checkbox" name="rule_Expanded_Arm_Flipping"> Expanded Arm Flipping</label>
        <label><input type="checkbox" name="rule_Front-Loaded_Deployment"> Front-Loaded Deployment</label>
        "##
    } else if query.ruleset.contains("Core Rules (2026)") {
        r##"
        <label><input type="checkbox" name="rule_Battlefield_Support_Assets" onchange="toggleBsp()"> Battlefield Support Assets</label>
        <label><input type="checkbox" name="rule_Battlefield_Support_Strikes" onchange="toggleBsp()"> Battlefield Support Strikes</label>
        <label><input type="checkbox" name="rule_Initiative_Die"> Initiative Die</label>
        "##
    } else if query.ruleset.contains("Alpha Strike") {
        r##"
        <label><input type="checkbox" name="rule_Multiple_Attack_Rolls"> Multiple Attack Rolls</label>
        <label><input type="checkbox" name="rule_Variable_Damage"> Variable Damage</label>
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
        .map(|(k, _)| k.replace("rule_", "").replace("_", " "))
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
        pv: input.get("pv").cloned().unwrap_or_default(),
        bsp: input.get("bsp").cloned().unwrap_or_default(),

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
