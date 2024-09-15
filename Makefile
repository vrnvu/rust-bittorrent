.PHONY: build_all build_bt build_tracker run_bt run_tracker stop_bt stop_tracker clean_all

build_all: build_bt build_tracker

build_bt:
	docker build -t bt -f bt/Dockerfile .

build_tracker:
	docker build -t tracker -f tracker/Dockerfile .

run_bt:
	docker run --rm --name bt bt:latest

run_tracker:
	docker run --rm --name tracker -p 3030:3030 tracker:latest

stop_bt:
	docker stop bt

stop_tracker:
	docker stop tracker

clean_bt:
	docker rm -f bt

clean_tracker:
	docker rm -f tracker

clean_all: clean_bt clean_tracker
