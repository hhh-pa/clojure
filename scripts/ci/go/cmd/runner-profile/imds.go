package main

import (
	"net/http"
	"strings"
)

// azureVMSize asks a cloud instance-metadata endpoint for the VM size.
// Any failure reads "unknown"; the production URL is the link-local
// Azure IMDS address (ubuntu runners).
func azureVMSize(client *http.Client, url string) string {
	request, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return "unknown"
	}
	request.Header.Set("Metadata", "true")
	response, err := client.Do(request)
	if err != nil {
		return "unknown"
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return "unknown"
	}
	body := make([]byte, 256)
	size := 0
	for size < len(body) {
		done, err := response.Body.Read(body[size:])
		size += done
		if err != nil {
			break
		}
	}
	if trimmed := strings.TrimSpace(string(body[:size])); trimmed != "" {
		return trimmed
	}
	return "unknown"
}
