import { $, component$, useSignal } from "@builder.io/qwik";

const EMAIL_REGEX = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const PERSONAL_EMAIL_DOMAINS = [
  "gmail.com",
  "yahoo.com",
  "hotmail.com",
  "outlook.com",
  "aol.com",
  "icloud.com",
  "mail.com",
  "protonmail.com",
  "yandex.com",
  "zoho.com",
];

export const BookDownloadForm = component$(() => {
  const email = useSignal("");
  const message = useSignal("");
  const isLoading = useSignal(false);
  const downloadUrl =
    "https://endflakytests.com/OReilly_Extending_Bazel_Nativelink.pdf";

  const handleSubmit = $(async () => {
    // Reset message before validation
    message.value = "";

    // Validate email
    if (!EMAIL_REGEX.test(email.value)) {
      message.value = "Please enter a valid email address";
      return;
    }

    const domain = email.value.split("@")[1]?.toLowerCase();
    if (domain && PERSONAL_EMAIL_DOMAINS.includes(domain)) {
      message.value = "Please use your work email address";
      return;
    }

    isLoading.value = true;

    try {
      // Simulate API call delay
      await new Promise((resolve) => setTimeout(resolve, 500));

      const response = await fetch("/api/mail.json", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          email: email.value,
          source: "oreilly-bazel-book-download",
          metadata: {
            bookTitle: "Extending Bazel to Its Full Potential",
            downloadTime: new Date().toISOString(),
          },
        }),
      });

      const data = await response.json();
      if (!response.ok) {
        throw new Error(data.error || "Failed to process request");
      }

      message.value = "Success! Your download will start automatically...";

      setTimeout(() => {
        const link = document.createElement("a");
        link.href = downloadUrl;
        link.download = "OReilly_Extending_Bazel_Nativelink.pdf";
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
      }, 1000);

      // Clear form after successful submission
      email.value = "";
    } catch (error) {
      message.value = "An unexpected error occurred. Please try again later.";
      console.error("Error processing download request:", error);
    } finally {
      isLoading.value = false;
    }
  });

  return (
    <div class="bg-gray-50 dark:bg-gray-900 rounded-2xl p-8 max-w-md mx-auto">
      <h3 class="text-2xl font-semibold mb-4 text-center">
        Get Your Free Copy
      </h3>
      <p class="text-gray-600 dark:text-gray-400 mb-6 text-center">
        Enter your work email to download the book
      </p>

      <form onSubmit$={(e) => e.preventDefault()} class="space-y-4">
        <div>
          <label class="block text-sm font-medium mb-2" for="book-email">
            Work Email Address *
          </label>
          <input
            id="book-email"
            name="email"
            type="email"
            placeholder="you@company.com"
            bind:value={email}
            required={true}
            autocomplete="email"
            disabled={isLoading.value}
            class="w-full px-4 py-3 rounded-lg border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-800 text-gray-900 dark:text-white focus:ring-2 focus:ring-purple-500 focus:border-transparent transition-colors"
          />
        </div>

        {message.value && (
          <div
            class={`text-sm text-center p-3 rounded-lg ${
              message.value.includes("Success")
                ? "bg-green-100 text-green-700 dark:bg-green-900 dark:text-green-300"
                : "bg-red-100 text-red-700 dark:bg-red-900 dark:text-red-300"
            }`}
          >
            {message.value}
          </div>
        )}

        <button
          type="button"
          onClick$={handleSubmit}
          disabled={isLoading.value}
          class="w-full py-3 px-4 bg-gradient-to-r from-purple-600 to-blue-600 text-white font-semibold rounded-lg hover:from-purple-700 hover:to-blue-700 transition-all duration-200 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {isLoading.value ? "Processing..." : "Download Book"}
        </button>

        <p class="text-xs text-gray-500 dark:text-gray-400 text-center">
          By downloading, you agree to receive occasional updates from
          NativeLink.
        </p>
      </form>
    </div>
  );
});
