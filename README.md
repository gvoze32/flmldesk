# FL ML Desk

A simple Windows app for editing your **Football Life 2026** or **PES 2021**
Master League save while your career is in progress.

What you can do:

- **Sign players for your club.** You choose the transfer fee, the yearly
  salary and when the contract ends. The fee is taken from your transfer
  budget. The deal is agreed like in the game: the player shows in your
  negotiations and the game moves him to your squad on the next in-game day.
- **Sell players from your club.** Their contract ends and the fee goes into
  your transfer budget.
- **Move players between other (AI) clubs.**
- **Change your finances:** transfer budget, yearly salary budget and manager
  name.

The app tells you when a new version is available.

## Download

Go to the [Releases](../../releases) page and download
`FLMLDesk-vX.Y.Z-windows-x64.exe`. Nothing to install: just open it.

## How to use

1. In the game, save your Master League and **go back to the main menu or close
   the game**.
2. Open `FLMLDesk-vX.Y.Z-windows-x64.exe`.
3. Click **Open ML save…**, or drag your save file onto the window. Saves are in
   `Documents\KONAMI\eFootball PES 2021 SEASON UPDATE\<folder>\save\`
   and are named like `ML00000001`.
   - The app finds your club and the in-game date by itself.
4. **Game folder (names/OVR)…** (only once): choose the folder where the game is
   installed. This fills in the names, positions, ages and OVR of all players.
5. **Player transfers** tab: search for a player, fill in **Transfer fee**,
   **Salary per year** and **Contract until** (or click **+1 yr** to **+5 yr**
   from the in-game date), then press **Transfer →** below.
   - You can type amounts like `12,500,000`, `12.5m` or `750k`.
   - If you pick one of your own players, the app switches to selling.
   - A player you are already negotiating for in the game is refused: finish
     or cancel that negotiation in the game first.
   - A player without a club (free agent) also arrives the next day; in the
     game he only negotiates his salary, so leave the fee at 0.
   - Saves the app can't read the transfer lists of: the player joins your
     squad right away instead.
6. **Budget & manager** tab: the **Finances** box holds both budgets, the
   **Manager** box the manager name; each has its own **Apply** button. Both
   budgets are shown as in the game: the salary budget is what is left after
   your club's salaries.
7. Click **Save (with automatic backup)**. A copy of your old save is kept in
   the `FLMLDesk_backup` folder next to it, just in case.
8. Load the save in the game and check your squad and finances.

You can undo changes one by one before saving. **Currency** only changes how
amounts are shown and typed; it uses the same exchange rates as the game.

## Good to know

- **Loans are not supported.** Every transfer is permanent.
- **Free agents:** players listed under "Others (national team, no club)" may
  be free agents in the game. Moving free agents with an editor is known to
  cause "ghost players", so check them in the game first.
- Selling has not been tested on many saves yet. After selling, check in the
  game that everything looks right.
- Contracts are only changed for your own club. Moves between AI clubs only
  change their squads.
- A club can have at most 40 players. Shirt numbers and line-ups are set in the
  game.
- The game's finance report won't list the fees paid through the app.
- On a PES 2021 save without the Football Life patch, budget editing has not
  been tested yet. If the app can't find the budget, it won't let you edit it.

## Credits

Credits and third-party licences: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
