function generateCryptoRandomState() {
    const randomValues = new Uint8Array(16);
    window.crypto.getRandomValues(randomValues);

    // Base64 encode the byte data (url safe)
    return btoa(String.fromCharCode(...randomValues))
        .replace(/\+/g, "-")
        .replace(/\//g, "_")
        .replace(/=+$/, "");
}

export function oauth2SignIn() {
    // create random state value and store in local storage
    const state = generateCryptoRandomState();
    localStorage.setItem("state", state);

    const params = new URLSearchParams({
        client_id:
            "124858368586-osnpmf9fn0aca69r41gmjif9jl766r5k.apps.googleusercontent.com",
        redirect_uri: new URL("/", window.location.href).toString(),
        scope: "https://www.googleapis.com/auth/youtube",
        state: state,
        response_type: "token",
    });

    // Google's OAuth 2.0 endpoint for requesting an access token
    window.location.href =
        `https://accounts.google.com/o/oauth2/v2/auth?${params.toString()}`;
}
