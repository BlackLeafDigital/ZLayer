# CronJobResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Enabled** | **bool** | Whether the job is enabled | 
**LastRun** | Pointer to **NullableString** | When the job last ran (ISO 8601 format) | [optional] 
**Name** | **string** | Job name | 
**NextRun** | Pointer to **NullableString** | Next scheduled run time (ISO 8601 format) | [optional] 
**Schedule** | **string** | Cron schedule expression | 

## Methods

### NewCronJobResponse

`func NewCronJobResponse(enabled bool, name string, schedule string, ) *CronJobResponse`

NewCronJobResponse instantiates a new CronJobResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCronJobResponseWithDefaults

`func NewCronJobResponseWithDefaults() *CronJobResponse`

NewCronJobResponseWithDefaults instantiates a new CronJobResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetEnabled

`func (o *CronJobResponse) GetEnabled() bool`

GetEnabled returns the Enabled field if non-nil, zero value otherwise.

### GetEnabledOk

`func (o *CronJobResponse) GetEnabledOk() (*bool, bool)`

GetEnabledOk returns a tuple with the Enabled field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEnabled

`func (o *CronJobResponse) SetEnabled(v bool)`

SetEnabled sets Enabled field to given value.


### GetLastRun

`func (o *CronJobResponse) GetLastRun() string`

GetLastRun returns the LastRun field if non-nil, zero value otherwise.

### GetLastRunOk

`func (o *CronJobResponse) GetLastRunOk() (*string, bool)`

GetLastRunOk returns a tuple with the LastRun field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastRun

`func (o *CronJobResponse) SetLastRun(v string)`

SetLastRun sets LastRun field to given value.

### HasLastRun

`func (o *CronJobResponse) HasLastRun() bool`

HasLastRun returns a boolean if a field has been set.

### SetLastRunNil

`func (o *CronJobResponse) SetLastRunNil(b bool)`

 SetLastRunNil sets the value for LastRun to be an explicit nil

### UnsetLastRun
`func (o *CronJobResponse) UnsetLastRun()`

UnsetLastRun ensures that no value is present for LastRun, not even an explicit nil
### GetName

`func (o *CronJobResponse) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CronJobResponse) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CronJobResponse) SetName(v string)`

SetName sets Name field to given value.


### GetNextRun

`func (o *CronJobResponse) GetNextRun() string`

GetNextRun returns the NextRun field if non-nil, zero value otherwise.

### GetNextRunOk

`func (o *CronJobResponse) GetNextRunOk() (*string, bool)`

GetNextRunOk returns a tuple with the NextRun field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNextRun

`func (o *CronJobResponse) SetNextRun(v string)`

SetNextRun sets NextRun field to given value.

### HasNextRun

`func (o *CronJobResponse) HasNextRun() bool`

HasNextRun returns a boolean if a field has been set.

### SetNextRunNil

`func (o *CronJobResponse) SetNextRunNil(b bool)`

 SetNextRunNil sets the value for NextRun to be an explicit nil

### UnsetNextRun
`func (o *CronJobResponse) UnsetNextRun()`

UnsetNextRun ensures that no value is present for NextRun, not even an explicit nil
### GetSchedule

`func (o *CronJobResponse) GetSchedule() string`

GetSchedule returns the Schedule field if non-nil, zero value otherwise.

### GetScheduleOk

`func (o *CronJobResponse) GetScheduleOk() (*string, bool)`

GetScheduleOk returns a tuple with the Schedule field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSchedule

`func (o *CronJobResponse) SetSchedule(v string)`

SetSchedule sets Schedule field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


