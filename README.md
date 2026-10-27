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
- **Loan players in or out.** You choose the loan fee and when the loan ends.
  The loan is agreed like in the game and the player moves on the next
  in-game day; his contract stays with the club that loans him out.
- **Move players between other (AI) clubs.**
- **Renew contracts:** give a player of your club a new yearly salary and
  contract end date.
- **Release players:** end a contract without a new club, optionally paying a
  compensation from your transfer budget.
- **See your squad's contracts:** salaries, end dates, contracts ending within
  12 months and the total wage bill.
- **Filter players** by position, OVR and age, and **export** the player list
  or your squad to a CSV file (opens in Excel).
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
   - **Position**, **OVR** and **Age** narrow the list (players whose OVR or
     age isn't known are hidden while a filter is on). **Export list (CSV)…**
     saves the list as shown.
   - Tick **Loan** to loan the player in (from an AI club) or out (one of
     your players), fill in **Loan fee** and **Loan until** (**31 Dec** or
     **30 Jun**, the dates the game uses), then press **Loan →**. There is no
     salary to enter: the game stores none for a loan.
   - If you pick one of your own players, the app switches to selling.
   - A player you are already negotiating for in the game is refused: finish
     or cancel that negotiation in the game first.
   - A player without a club (free agent) also arrives the next day; in the
     game he only negotiates his salary, so leave the fee at 0.
   - Saves the app can't read the transfer lists of: the player joins your
     squad right away instead.
6. **My squad & contracts** tab: your players sorted by contract end; contracts
   ending within 12 months are highlighted. Select a player:
   - **Apply contract** sets the new **Salary per year** and **Contract until**
     (**+1 yr** to **+5 yr** from the in-game date).
   - **Release player** ends his contract and takes him out of your squad.
     **Remaining salary** fills in what is still owed until the contract ends;
     with **Pay from transfer budget** it is taken from the budget.
   - **Export squad (CSV)…** saves the squad with salaries and contract dates.
7. **Budget & manager** tab: the **Finances** box holds both budgets, the
   **Manager** box the manager name; each has its own **Apply** button. Both
   budgets are shown as in the game: the salary budget is what is left after
   your club's salaries.
8. Click **Save (with automatic backup)**. A copy of your old save is kept in
   the `FLMLDesk_backup` folder next to it, just in case.
9. Load the save in the game and check your squad and finances.

You can undo changes one by one before saving. **Currency** only changes how
amounts are shown and typed; it uses the same exchange rates as the game.

## Good to know

- **Loans** haven't been tested in the game yet, and only up to the day the
  player moves: what the game does when a loan ends is unknown. Check the
  player and your budget in the game after loading. Players without a club,
  and loans between two AI clubs, can't be loaned.
- A player on loan at your club has no contract with you, so the
  **My squad & contracts** tab shows him as "no contract" and he can't be
  renewed, released or sold. A player you loaned out keeps his contract with
  you, so his salary still counts in the wages the app shows.
- **Free agents:** players listed under "Others (national team, no club)" may
  be free agents in the game. Moving free agents with an editor is known to
  cause "ghost players", so check them in the game first.
- **Selling, releasing and moving players between AI clubs** update the
  player's own data in the save the way the game does on a transfer: his
  previous club, join date, competition registrations, shirt number and your
  "My Team" list. **Renewing** also writes the new salary and end date into
  his data, which the game keeps next to the contract. The game registers
  moved players for competitions again; this has not been played through a
  season change yet, so check in the game after the next season starts.
- Selling or releasing a player from your starting eleven moves the next
  player in your "My Team" list up: check your line-up in the game.
- A sold or moved player keeps his shirt number when it is free at the new
  club, otherwise he gets the lowest free number. Players on loan can't be
  sold, released or moved by the app.
- If the app can't read the players' data in a save, selling, releasing,
  renewing and moving players between AI clubs are locked (a warning shows
  when the save opens); signings and loans still work.
- Contracts are only changed for your own club.
- A club can have at most 40 players. Line-ups are set in the game.
- The game's finance report won't list the fees paid through the app.
- On a PES 2021 save without the Football Life patch, budget editing has not
  been tested yet. If the app can't find the budget, it won't let you edit it.

## Credits

Credits and third-party licences: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
