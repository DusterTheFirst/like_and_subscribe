<script lang="ts">
    import { oauth2SignIn } from "./lib/oauth";

    const STORAGE_KEY = "las-access_token";
    const STATE_KEY = "las-state";

    const params = new URLSearchParams(location.hash.substring(1));

    const state = params.get("state");
    const access_token = params.get("access_token");
    if (state !== null && access_token !== null) {
        if (state == localStorage.getItem(STATE_KEY)) {
            localStorage.setItem(STORAGE_KEY, access_token);
            localStorage.removeItem(STATE_KEY);

            history.replaceState({}, "", location.pathname);

            trySampleRequest();
        } else {
            console.log("State mismatch. Possible CSRF attack");
        }
    }
    // TODO: error handling

    // If there's an access token, try an API request.
    // Otherwise, start OAuth 2.0 flow.
    async function trySampleRequest() {
        let access_token = localStorage.getItem(STORAGE_KEY);
        if (access_token == null) {
            oauth2SignIn();
            return;
        }

        let response = await fetch(
            "https://www.googleapis.com/youtube/v3/channels?part=snippet&mine=true",
            {
                headers: {
                    authorization: `Bearer ${access_token}`,
                },
            },
        );

        if (response.ok) {
            console.log(await response.json());
        } else if (response.status === 401) {
            // Token invalid, so prompt for user permission.
            oauth2SignIn();
        }
    }
</script>

<main>
    <button on:click={trySampleRequest}>Try sample request</button>
</main>

<style>
</style>
