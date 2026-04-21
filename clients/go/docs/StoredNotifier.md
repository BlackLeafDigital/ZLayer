# StoredNotifier

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Config** | [**NotifierConfig**](NotifierConfig.md) | Channel-specific configuration (webhook URL, SMTP settings, etc.). | 
**CreatedAt** | **string** | When the notifier was created. | 
**Enabled** | **bool** | Whether this notifier is active. Disabled notifiers are skipped. | 
**Id** | **string** | UUID identifier. | 
**Kind** | [**NotifierKind**](NotifierKind.md) | Notification channel type. | 
**Name** | **string** | Display name (e.g. &#x60;\&quot;deploy-alerts\&quot;&#x60;). | 
**UpdatedAt** | **string** | When the notifier was last updated. | 

## Methods

### NewStoredNotifier

`func NewStoredNotifier(config NotifierConfig, createdAt string, enabled bool, id string, kind NotifierKind, name string, updatedAt string, ) *StoredNotifier`

NewStoredNotifier instantiates a new StoredNotifier object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStoredNotifierWithDefaults

`func NewStoredNotifierWithDefaults() *StoredNotifier`

NewStoredNotifierWithDefaults instantiates a new StoredNotifier object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetConfig

`func (o *StoredNotifier) GetConfig() NotifierConfig`

GetConfig returns the Config field if non-nil, zero value otherwise.

### GetConfigOk

`func (o *StoredNotifier) GetConfigOk() (*NotifierConfig, bool)`

GetConfigOk returns a tuple with the Config field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetConfig

`func (o *StoredNotifier) SetConfig(v NotifierConfig)`

SetConfig sets Config field to given value.


### GetCreatedAt

`func (o *StoredNotifier) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *StoredNotifier) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *StoredNotifier) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetEnabled

`func (o *StoredNotifier) GetEnabled() bool`

GetEnabled returns the Enabled field if non-nil, zero value otherwise.

### GetEnabledOk

`func (o *StoredNotifier) GetEnabledOk() (*bool, bool)`

GetEnabledOk returns a tuple with the Enabled field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEnabled

`func (o *StoredNotifier) SetEnabled(v bool)`

SetEnabled sets Enabled field to given value.


### GetId

`func (o *StoredNotifier) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *StoredNotifier) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *StoredNotifier) SetId(v string)`

SetId sets Id field to given value.


### GetKind

`func (o *StoredNotifier) GetKind() NotifierKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *StoredNotifier) GetKindOk() (*NotifierKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *StoredNotifier) SetKind(v NotifierKind)`

SetKind sets Kind field to given value.


### GetName

`func (o *StoredNotifier) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *StoredNotifier) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *StoredNotifier) SetName(v string)`

SetName sets Name field to given value.


### GetUpdatedAt

`func (o *StoredNotifier) GetUpdatedAt() string`

GetUpdatedAt returns the UpdatedAt field if non-nil, zero value otherwise.

### GetUpdatedAtOk

`func (o *StoredNotifier) GetUpdatedAtOk() (*string, bool)`

GetUpdatedAtOk returns a tuple with the UpdatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpdatedAt

`func (o *StoredNotifier) SetUpdatedAt(v string)`

SetUpdatedAt sets UpdatedAt field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


