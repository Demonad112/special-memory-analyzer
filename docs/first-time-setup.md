# Deep Server Analyzer: first-time setup and tear-down

A step-by-step guide that assumes you have never done this before. It covers getting the app from
GitHub, moving it to another computer, installing it, and removing everything again. It does not cover
how to use the app once it is open.

Only do this on computers you are allowed to change.

## What you are about to do

The app's installer is signed with a one-off certificate that Windows does not know yet. So the kit
tells Windows to trust that certificate for a few hours, installs the app, and then takes the trust
away again. You control the timer, and the red button removes the trust at any moment.

## Part 1: Download from GitHub

1. Open a web browser and go to the project's **Releases** page:
   `https://github.com/Demonad112/special-memory-analyzer/releases`
2. The top entry marked **Latest** is the newest version. Click its title.
3. Scroll down to **Assets** and click these files to download them:
   - `Deep-Server-Analyzer-usb-kit.zip` (the one you need)
   - `SHA256SUMS.txt` and `dsa-signing.cer` (optional, for checking)
4. Your browser saves them in your **Downloads** folder.
5. Leave the release page open. Near the top of the notes there is a line starting
   **Thumbprint (SHA-1)**. You will compare it with one the installer shows.

If Edge or Chrome says the file "isn't commonly downloaded", click the three dots, choose **Keep**,
then **Keep anyway**. That happens with any new file.

## Part 2: Move it to the other computer

1. Plug in a USB stick.
2. Open **File Explorer**, then **Downloads**.
3. Right-click `Deep-Server-Analyzer-usb-kit.zip` and choose **Copy**.
4. Open the USB stick in File Explorer, right-click an empty space and choose **Paste**.
5. Wait for the copy to finish, then eject the stick: click the USB icon in the bottom-right corner
   of the screen (it may be under the **^** arrow) and choose **Eject**.
6. Plug the stick into the other computer.
7. Open the stick in File Explorer and **copy** the zip onto that computer's Desktop or Downloads
   folder. Do not run anything from inside the zip.

## Part 3: Unzip it into its own folder

The scripts do not work from inside a zip, so this step matters.

1. Right-click the zip, choose **Properties**, tick **Unblock** at the bottom if you see it, and click
   **OK**. This removes the "downloaded from the internet" mark so Smart App Control does not block the
   scripts. If there is no **Unblock** box, skip this.
2. Right-click the zip and choose **Extract All...**
3. Click **Extract**. A normal folder with the same name opens.
4. Check that the folder contains at least these files:
   - `DSA-Installer.cmd`
   - `Deep Server Analyzer_...-setup.exe`
   - `dsa-signing.cer`
   - `thumbprint.txt`
   - `SHA256SUMS.txt`

You can leave the folder on the Desktop or move it anywhere you like. You can move the whole folder
and the kit still works.

## Part 4: Install and trust

1. Double-click **`DSA-Installer.cmd`**.
2. Windows may show a blue **Windows protected your PC** box. Click **More info**, then
   **Run anyway**. This appears because the file came from the internet.
3. Windows asks **Do you want to allow this app to make changes?** Click **Yes**. This is expected:
   the installer needs administrator rights to trust the certificate.
4. A small dark window opens, titled **Deep Server Analyzer installer**.
5. Compare the start of the thumbprint in the window (for example `ABCD14B8D400...`) with the
   thumbprint on the release page. They must match. If they do not, close the window and stop.
6. Click the blue **Install** button.
7. A progress bar shows while the kit checks its own files, trusts the certificate and installs the
   app. This takes a few seconds to a minute.
8. When it finishes the window shows **Installed** and a large clock counting down, for example
   `07:59:45`. That is how long Windows will trust the certificate. By default it is 8 hours.

If it says **Install failed**, read the message in the window. Nothing stays trusted after a failure.
The details are in `C:\ProgramData\DSA\trust.log`.

## Part 5: Use the app

1. Open the **Start** menu and type **Deep Server Analyzer**, then press **Enter**.
2. The app runs like any other program. See the app's own screens for scanning and comparing.

You can close the dark installer window while you work. The countdown keeps running in the
background and removes the trust by itself when it reaches zero.

## Part 6: Adding time while you work

Still need the trust window open, for example to reinstall or install on another account?

1. Double-click `DSA-Installer.cmd` again. It opens straight to the countdown.
2. Click the blue **▲ +1 h** button. Each click adds one hour, up to 72 hours from now.

## Part 7: Tear it down

Do this when you are finished. It takes a minute.

### Remove the trust (the important part)

1. Double-click `DSA-Installer.cmd` and click **Yes** on the prompt.
2. Click the red **✕ Remove trust** button.
3. A blue PowerShell window opens and removes the certificate. It then prints a check list:

   ```text
   OK    Certificate absent from LocalMachine\Root
   OK    Certificate absent from LocalMachine\TrustedPublisher
   OK    Scheduled task DSA-TrustExpiry is gone
   OK    Expiry record is gone
   ```

   All four lines must say **OK** in green and the last line should read **All clear**.

4. Press any key. The installer window turns green and says **Trust removed and verified**.
5. Click **Close**.

If any line says **FAIL** in red, run `Remove-Trust.cmd` from the same folder (click **Yes** on the
prompt) and read what it prints. The log is `C:\ProgramData\DSA\trust.log`.

If you do nothing, the trust is removed automatically when the countdown ends. The red button just
does it sooner and shows you proof.

### Uninstall the app (only if you want it gone)

Removing the trust does not remove the app. To uninstall it too:

1. Open **Settings**, then **Apps**, then **Installed apps**.
2. Find **Deep Server Analyzer**, click the three dots, choose **Uninstall** and confirm.

### Tidy up the files (optional)

1. Delete the extracted kit folder and the zip.
2. Delete the folder `C:\ProgramData\DSA`. It only holds the log and a copy of the removal script. It
   is a hidden folder: in File Explorer, open **This PC**, then **Local Disk (C:)**, then type
   `ProgramData` into the address bar.
3. Empty the Recycle Bin if you want the files gone for good.

## Quick problem guide

| What you see                                 | What to do                                                                                                    |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| "Smart App Control blocked a file"           | Unblock the files: in PowerShell run `Get-ChildItem "<kit folder>" -Recurse \| Unblock-File`, then try again. |
| Install button is greyed out                 | You opened the file from inside the zip or from a partial copy. Extract the whole zip first.                  |
| The window never opens                       | Click **Yes** on the administrator prompt. If you clicked **No**, double-click it again.                      |
| Thumbprint does not match the release page   | Stop. Download the zip again from the real Releases page.                                                     |
| Install failed                               | Read the message, then see `C:\ProgramData\DSA\trust.log`. Nothing stays trusted after a failure.             |
| Company security software blocks the scripts | Ask the security team first. The kit changes certificate stores and adds a scheduled task.                    |
| Not sure whether anything is still trusted   | Run the red button or `Remove-Trust.cmd`. It is safe to run when nothing is trusted.                          |

For the technical details behind all of this, see [usb-install.md](usb-install.md).
