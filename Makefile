PORT ?= 8000

.PHONY: serve

# Serve the static site at http://localhost:$(PORT)
serve:
	python3 -m http.server $(PORT) -d site
